# Patching the stock MCU firmware

`patch.py` puts the telnet hook into the stock Zeppelin Air MCU app (2.04.05, `blobs/mcu/zd1-2.04.05.hex`): once the
module is ready, the MCU writes reg 0x75 = "23" and reg 0x76 = 5, the module's KnOS shell on telnet port 23,
re-asserted every ~30 s (`telnet_hook.S`). Python only:

```
python3 -I stock-patch/patch.py out.hex                       # ZD1 2.04.05 -> a .hex for DFU
python3 -I stock-patch/patch.py --fw zd1lv20435.fw out.fw     # ZD1L 2.04.35 -> a repacked B&W .fw
python3 -I stock-patch/patch.py --fw a7v20477.fw out.fw       # A7 2.04.77 -> a repacked B&W .fw
```
Flashed and checked on the unit 2026-10-16 (2.04.05). It is not on the unit now: that runs our own firmware (`mcu/`),
which has the amp interlock built in and puts the shell on telnet with `zd1 reg write` (`docs/module/knos.md`).

## Three builds, one blob
`voTask` (0x8000e19c) and `WRITEREG` (0x8000b910) keep the same address in ZD1 2.04.05, ZD1L 2.04.35 and A7 2.04.77,
so the hook's code is identical across them; only four literals it uses are build-specific. `patch()` keys on the
crt0 bss-end value (`memend`, the word at 0x80008088) and relocates those four in the committed blob — the module-comm
`STATUS` word and attach-state byte the hook gates on, and the two carved-RAM addresses, which follow `memend`:

| `memend` | build | `STATUS` | `ATTACH` | carve | heap literal |
|---|---|---|---|---|---|
| 0x8250 | ZD1 2.04.05 / ZD1L 2.04.35 | 0x3ee0 | 0x4df4 | 0x8250 | by value (0x8002ae28, or 0x8002ae24 on ZD1L: the -4 data shift) |
| 0x7a08 | A7 2.04.77 | 0x3918 | 0x482c | 0x7a08 | by value (0x8002a3ec) |

The voTask call pointer (`== voTask`'s address, unique in flash) and sbrk's heap-base literal (`== memend`, unique in
0x8002A000-0x8002B000) are located by value, not address, so the −0x5c8 RAM shift and the moved pointer in A7
(0x8001e870 -> 0x8001de48) need no hardcoding. An unrecognised build (unknown `memend`, e.g. the older, structurally
different ZD1L 2.01.04) is refused rather than patched wrong. `--fw` patches the MCU `.dmb` inside a B&W over-the-air
`.fw` (`tools/zeppelin/fw/bwfw.py`) and repacks it, the module `.bcd` untouched; the `.dmb` links at 0x80008000, so
there is room for the hook at the cave (free in all three: each app ends well before it).

What the emulator showed for A7 [EMU 2026-11-14: `zemu run module.bcd --mcu FLASH --renode 'zd1:board Adc "2:0x100"'`,
the `.fw` split by `bwfw.py`]:
- The patched MCU boots on the ZD1 board model ("Device: AZ1") and reaches RUN identically to stock, with the 8-byte
  carve live (free heap 16812 B vs stock 16820 B); the A7 module image boots the full KnOS stack and gets networking.
- **The hook fires.** Watching the carved STATE byte (`zd1:board Watch "0x7a08:4"`) shows it advance 0 → 1 → 2 → 3 → 4
  over t=5.3-6.3 s — the whole write sequence (reg 0x75 = "23", reg 0x76 = 5), reaching the enabled state. For the
  state machine to leave state 0 it must have seen `STATUS(0x3918) == 0x2100` and `ATTACH(0x482c) == 1`, so the
  relocated A7 addresses are right and the gate opens. Telnet port 23 then accepts a TCP connection (`net.log`:
  `tcp open host -> :23`).
- Interactive shell output over the connection is empty in the emulator — but **identically for the known-good
  ZD1L**, so it is an emulator usernet / shell-routing limitation, not an A7 or patch issue.
- Both stock and patched A7 (and ZD1L) log a *pre-existing, non-fatal* assert at ~21 s (`dmhBcoMgr_ercGetCneValue`,
  `dmhBcoMgr_Cne.c:149`): the MCU fails to read one CnE value from the module because the emulator boots the image
  against the test unit's PersParam (NAND 0x20000) and an uninitialised persistent-CnE region (NAND 0x60000,
  `fixpage result=6`). The hook fires straight through it. A real A7 unit, with its own factory data, would not hit
  it.

So the A7 payload is built, verified static, and verified to fire end-to-end up to the listening telnet port; a live
interactive shell is unproven in the emulator (the ZD1L limitation above) and awaits an A7 unit (`docs/backlog.md`).
Delivery is the other open part: what on a ZD1L or an A7 takes a `.fw`'s MCU part is unconfirmed
(`docs/mcu/stock-firmware.md`), so these are the payload for that route, not proof the route exists.

How: the BcoWrapper tick calls `voTask` through a pointer literal; the patch points it at the hook in free flash at
0x80038100, which runs a small state machine and then calls the real `voTask`. One byte of state at RAM `memend`
(crt0's bss end and the heap base move +8). Every patched word is checked against the stock value first. Nothing in
the bootloader or the user page changes, so DFU recovery stays available.

Flash: `zd1 dfu write out.hex --backup <backup.tar> --yes --start` (`zd1 dfu backup` first) (DFU: hold ON/OFF at power-up).
Back to stock: the same with `blobs/mcu/zd1-2.04.05.hex`.

The hook blob (`telnet_hook.bin`) is committed; `build.sh` rebuilds it from the source (`nix build .#telnet-hook`)
and shows a diff if anything changed.
