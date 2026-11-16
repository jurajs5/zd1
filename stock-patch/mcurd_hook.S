/*
 * Extended telnet + MCU-flash-read hook for the stock B&W Zeppelin Air MCU firmware
 * (ZD1_v2-04-05 / ZD1L_v2-04-35 / A7_v2-04-77, AT32UC3A1256, AVR32 UC3A).
 *
 * A SUPERSET of telnet_hook.S: it keeps the ~30 s telnet re-assert exactly, and adds a register-mailbox step so the
 * host can read the MCU's own internal flash -- the 16 KB bootloader at 0x80000000 among it -- over SPI, the
 * reconnaissance phase of issue #1 ("read the MCU flash over SPI"). Reading never writes flash: like `fburn rd`,
 * it only streams bytes out, so there is no interlock and no way-back concern.
 *
 * Mechanism (docs/mcu-bootloader-dump.md, the contract the host zair/mcuflash.py already speaks):
 *   - CMD register 0x70 (module-served, 2 bytes): a big-endian u16 block index i.
 *   - RSP register 0x71 (BLOCK+4 bytes): [i: u16 BE][len: u16 BE][data: BLOCK]. Filled only after block i is read,
 *     so the host knows the data is fresh when the echoed index matches.
 *   - BASE = 0x80000000, BLOCK = 64. Block i is BASE + i*BLOCK; the whole 256 KB is i = 0..4095, the bootloader
 *     i = 0..255.
 * The stock register type table at 0x8000a27c does not type 0x70/0x71 (they are neither F nor V), so patch.py sets
 * table[0x70] = F (2-byte fixed) and table[0x71] = V (variable) in flash before splicing this blob.
 *
 * Both register primitives are stock functions, reached exactly as the plain hook reaches WRITEREG from the voTask
 * tick (proven on hardware):
 *   READREG  0x8000b860  dmhBcoMgr_ercReadRegister(reg, buf, max, *len)  -> status in r12, *len set
 *   WRITEREG 0x8000b910  dmhBcoMgr_ercWriteRegister(reg, buf, len)       -> status in r12
 * The mailbox step is rate-limited to ~0.25 s (the host polls RSP every 0.3 s) so it does not clock an SPI read on
 * every main-loop tick.
 *
 * TODO before hardware: this builds and is designed from static RE; verify in the emulator (emu/zemu run --mcu) that
 *  (1) ercReadRegister is reentrancy-safe from the voTask tick (the plain hook proves ercWriteRegister is), and
 *  (2) the read-slot contention noted in docs/protocol/spi-registers.md §6 does not need the "idle read slot" guard.
 *
 * Persistent RAM (carved and zeroed by crt0 to 0x8268 by patch.py):
 *   STATE byte  @0x8250 : telnet write sequence 0..3, 4 = enabled/counting   (as telnet_hook.S)
 *   LASTCOUNT   @0x8254 : COUNT snapshot for the 30 s telnet re-assert        (as telnet_hook.S)
 *   LASTPOLL    @0x8258 : COUNT snapshot for the mailbox poll gate
 *   MBFLAGS byte@0x825c : b0 = at least one index has been served
 *   LASTIDX     @0x8260 : the last block index served (so each index is served once)
 */

	.text
	.align	2
	.global	telnet_hook
	.type	telnet_hook,@function

	.set	STATUS,    0x00003ee0	/* DM1000Status word, == 0x2100 when ready */
	.set	ATTACH,    0x00004df4	/* attach-state byte, == 1 in RUN          */
	.set	STATE,     0x00008250
	.set	LASTCOUNT, 0x00008254
	.set	LASTPOLL,  0x00008258
	.set	MBFLAGS,   0x0000825c
	.set	LASTIDX,   0x00008260
	.set	WRITEREG,  0x8000b910	/* dmhBcoMgr_ercWriteRegister(reg,buf,len) */
	.set	READREG,   0x8000b860	/* dmhBcoMgr_ercReadRegister(reg,buf,max,*len) */
	.set	VOTASK,    0x8000e19c	/* dmhBcoMgr_voTask (original call target) */
	.set	READY,     0x00002100
	.set	QUEUED,    0x00420020	/* ercWriteRegister: newly queued */
	.set	PERIOD,    0x76000000	/* ~1.98e9 cycles ~= 30 s at 66 MHz */
	.set	MBPERIOD,  0x01000000	/* ~16.8e6 cycles ~= 0.25 s at 66 MHz */
	.set	CMD_REG,   0x70
	.set	RSP_REG,   0x71
	.set	BLOCK,     64
	.set	FBASE,     0x80000000
	.set	IDLE_FLAG, 0x00000078	/* dmhBcoMgr access-path idle flag: 1 = idle, no SPI transaction in flight */
	.set	LOCK_FLAG, 0x00000079	/* register-access lock byte: 0 = unlocked (boLockRegisterAccess @0x8000b7a0) */

telnet_hook:
	/* --- gate: only act when the module is ready AND attached --- */
	lddpc	r8, .Lstatus
	ld.w	r9, r8[0]
	lddpc	r10, .Lready
	cp.w	r9, r10
	brne	.Lret			/* not ready: skip telnet AND mailbox */
	lddpc	r8, .Lattach
	ld.ub	r9, r8[0]
	cp.w	r9, 1
	brne	.Lret

	lddpc	r8, .Lstate
	ld.ub	r9, r8[0]
	cp.w	r9, 4
	brne	.Lactive		/* states 0-3 drive the telnet write sequence */

	/* --- state 4: telnet enabled; re-arm every PERIOD cycles --- */
	mfsr	r9, 0x108		/* COUNT */
	lddpc	r8, .Llastcount
	ld.w	r10, r8[0]
	sub	r9, r10
	lddpc	r10, .Lperiod
	cp.w	r9, r10
	brlo	.Lmailbox		/* not time yet: telnet stays up, go poll the mailbox */
	lddpc	r8, .Lstate
	mov	r9, 0
	st.b	r8[0], r9		/* re-arm: STATE = 0 */
	rjmp	.Lmailbox

.Lactive:
	/* --- states 0-3 call ercWriteRegister: save LR, keep SP 8-byte aligned --- */
	pushm	lr
	sub	sp, 4
	lddpc	r8, .Lstate
	ld.ub	r9, r8[0]

	cp.w	r9, 0
	brne	.Lc1
	lddpc	r8, .Lwritereg
	mov	r12, 0x75
	lddpc	r11, .Lport
	mov	r10, 2
	icall	r8
	lddpc	r9, .Lqueued
	cp.w	r12, r9
	brne	.Lrestore
	mov	r9, 1
	rjmp	.Lsave

.Lc1:
	cp.w	r9, 1
	brne	.Lc2
	lddpc	r8, .Lwritereg
	mov	r12, 0x75
	lddpc	r11, .Lport
	mov	r10, 2
	icall	r8
	cp.w	r12, 0
	brne	.Lrestore
	mov	r9, 2
	rjmp	.Lsave

.Lc2:
	cp.w	r9, 2
	brne	.Lc3
	lddpc	r8, .Lwritereg
	mov	r12, 0x76
	lddpc	r11, .Lmode
	mov	r10, 2
	icall	r8
	lddpc	r9, .Lqueued
	cp.w	r12, r9
	brne	.Lrestore
	mov	r9, 3
	rjmp	.Lsave

.Lc3:
	lddpc	r8, .Lwritereg
	mov	r12, 0x76
	lddpc	r11, .Lmode
	mov	r10, 2
	icall	r8
	cp.w	r12, 0
	brne	.Lrestore
	mfsr	r9, 0x108
	lddpc	r8, .Llastcount
	st.w	r8[0], r9
	mov	r9, 4

.Lsave:
	lddpc	r8, .Lstate
	st.b	r8[0], r9

.Lrestore:
	sub	sp, -4
	ldm	sp++, lr
	/* fall through to the mailbox */

	/* ================= mailbox: read one MCU-flash block on request ================= */
.Lmailbox:
	/* rate-limit: act only every MBPERIOD cycles */
	mfsr	r9, 0x108
	lddpc	r8, .Llastpoll
	ld.w	r10, r8[0]
	sub	r9, r10
	lddpc	r10, .Lmbperiod
	cp.w	r9, r10
	brlo	.Lret

	/* --- only read while the SPI access path is quiescent ---
	 * ercReadRegister does a synchronous read, but the register tasks (ticked from voTask, cooperative) keep a read
	 * transaction in flight ACROSS ticks (header, 5 ms, receive). Reading now would corrupt an in-flight transfer,
	 * which is what trips the module version check -> stock system error 7 [EMU 2026-11-16]. So read only when the
	 * access path is idle (0x78 == 1) and not locked (0x79 == 0) -- the "only while idle" the HW precedent used
	 * (docs/protocol/spi-registers.md §6). LASTPOLL advances only once we commit to a read, so an idle window is
	 * retried every tick, not once per MBPERIOD. */
	lddpc	r8, .Lidle
	ld.ub	r9, r8[0]
	cp.w	r9, 1
	brne	.Lret			/* access task busy: a transaction is in flight */
	lddpc	r8, .Llock
	ld.ub	r9, r8[0]
	cp.w	r9, 0
	brne	.Lret			/* register access locked by another path */

	mfsr	r9, 0x108
	lddpc	r8, .Llastpoll
	st.w	r8[0], r9		/* LASTPOLL = COUNT */

	/* frame: lr(4) + 12 = 16 (8-aligned). sp+0..1 = read buf, sp+4..7 = read len */
	pushm	lr
	sub	sp, 12

	/* ercReadRegister(0x70, buf=sp, max=2, *len=sp+4) */
	mov	r12, CMD_REG
	mov	r11, sp
	mov	r10, 2
	mov	r9, sp
	sub	r9, -4			/* r9 = sp + 4 */
	lddpc	r8, .Lreadreg
	icall	r8

	cp.w	r12, 0			/* status ok? */
	brne	.Lmbdone
	ld.w	r9, sp[4]		/* len */
	cp.w	r9, 2
	brlo	.Lmbdone

	/* i = (buf[0] << 8) | buf[1]  (big-endian u16) */
	ld.ub	r10, sp[0]
	lsl	r10, 8
	ld.ub	r11, sp[1]
	or	r10, r11		/* r10 = i */

	/* new index? (serve each index once) */
	lddpc	r8, .Lmbflags
	ld.ub	r9, r8[0]
	cp.w	r9, 0
	breq	.Lservice		/* nothing served yet */
	lddpc	r8, .Llastidx
	ld.w	r9, r8[0]
	cp.w	r9, r10
	breq	.Lmbdone		/* same index already served; RSP still holds it */

.Lservice:
	/* response frame: 72 bytes (8-aligned), holds [i:2][len:2][data:64] in the first 68 */
	sub	sp, 72
	/* header */
	mov	r11, r10
	lsr	r11, 8
	st.b	sp[0], r11		/* i >> 8 */
	st.b	sp[1], r10		/* i & 0xff */
	mov	r11, 0
	st.b	sp[2], r11		/* len >> 8 (BLOCK < 256) */
	mov	r11, BLOCK
	st.b	sp[3], r11		/* len & 0xff */
	/* copy BLOCK bytes from FBASE + i*BLOCK to sp+4 */
	lddpc	r11, .Lfbase
	mov	r9, r10
	lsl	r9, 6			/* i * 64 */
	add	r11, r9			/* r11 = src */
	mov	r12, sp
	sub	r12, -4			/* r12 = dst = sp + 4 */
	mov	r9, BLOCK/4		/* 16 words */
.Lcopy:
	ld.w	r8, r11++
	st.w	r12++, r8
	sub	r9, 1
	brne	.Lcopy

	/* record (before the call, which clobbers the caller-saved regs): LASTIDX = i, have_last = 1 */
	lddpc	r8, .Llastidx
	st.w	r8[0], r10
	lddpc	r8, .Lmbflags
	mov	r9, 1
	st.b	r8[0], r9

	/* ercWriteRegister(0x71, buf=sp, len=BLOCK+4) */
	mov	r12, RSP_REG
	mov	r11, sp
	mov	r10, BLOCK + 4
	lddpc	r8, .Lwritereg
	icall	r8

	sub	sp, -72			/* free the response frame */

.Lmbdone:
	sub	sp, -12
	ldm	sp++, lr

.Lret:
	lddpc	pc, .Lvotask

	.align	2
.Lstatus:	.long	STATUS
.Lattach:	.long	ATTACH
.Lstate:	.long	STATE
.Llastcount:	.long	LASTCOUNT
.Llastpoll:	.long	LASTPOLL
.Lmbflags:	.long	MBFLAGS
.Llastidx:	.long	LASTIDX
.Lwritereg:	.long	WRITEREG
.Lreadreg:	.long	READREG
.Lvotask:	.long	VOTASK
.Lready:	.long	READY
.Lqueued:	.long	QUEUED
.Lperiod:	.long	PERIOD
.Lmbperiod:	.long	MBPERIOD
.Lfbase:	.long	FBASE
.Lidle:		.long	IDLE_FLAG
.Llock:		.long	LOCK_FLAG
.Lport:		.long	.Lportdata
.Lmode:		.long	.Lmodedata

	.align	2
.Lportdata:	.byte	0x32, 0x33	/* "23" */
	.align	2
.Lmodedata:	.byte	0x05, 0x00	/* UARTPortsSelect mode 5 */

	.size	telnet_hook, .-telnet_hook
