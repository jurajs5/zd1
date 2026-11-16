# The extended telnet + MCU-flash-read hook (stock-patch/mcurd_hook.S), a superset of telnet-hook: it keeps the telnet
# re-assert and adds the register mailbox that reads the MCU's own flash over SPI (docs/mcu-bootloader-dump.md). Same
# AVR32 toolchain and linker script (hook.ld, ENTRY telnet_hook at the 0x80038100 cave).
{
  stdenvNoCC,
  avr32-toolchain,
  src,
}:
stdenvNoCC.mkDerivation {
  name = "zd1-mcurd-hook";
  src = src [
    "stock-patch/mcurd_hook.S"
    "stock-patch/hook.ld"
  ];
  sourceRoot = "source/stock-patch";
  nativeBuildInputs = [ avr32-toolchain ];
  buildPhase = ''
    avr32-as -march=ucr2 -o mcurd_hook.o mcurd_hook.S
    avr32-ld -T hook.ld -o mcurd_hook.elf mcurd_hook.o
    avr32-objcopy -O binary mcurd_hook.elf mcurd_hook.bin
  '';
  installPhase = "install -Dm644 -t $out mcurd_hook.bin";
}
