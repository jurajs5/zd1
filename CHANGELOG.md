# Changelog — our Zeppelin Air (ZD1L) firmware

Our firmware for the B&W Zeppelin Air with Lightning, on the fork `jurajs5/zd1`, branch
`zd1l-show` (on top of the maintainer's ZD1L support, `pawlowskialex/zd1` branch `zd1l`).

Versions are `0.1`, `0.2`, … (our own line, kept separate from upstream's `v0.1.x` tags).
Each version is a git tag on the commit the flashed `.fw` was built from.

## 0.1 — 2026-11-18 (commit `21d8846`) — on the speaker now

The working baseline: AirPlay 2, Spotify Connect, USB audio and automatic source switching
all play, plus our two additions (both pure MCU, `mcu/src/`). Confirmed on the unit.

(An earlier candidate, `d284ad3`, added a deferred pre-switch volume fade; on the unit source
switching misbehaved afterwards, so it was dropped and is not part of 0.1.)

- **Beat-reactive LED light show.** While a source plays, the LED shows that input's own
  colour for ~5 s after a switch (AirPlay white / Spotify green / USB blue), then runs a show
  whose brightness auto-ranges to the music's own recent window (so it shows real dynamics at
  any level) and whose colour jumps to a fresh random hue on each beat. Driven by the DSP's
  input peak meter, newly sampled for every source (`source_vis_level`). (`mcu/src/led.c`,
  `mcu/src/source.c`)
- **Less pop when switching inputs.** On a source switch the amp is muted *first* (before the
  DSP volume drop, DAC mute and input-mux switch) and held muted 80 ms before the unmute.
  (`mcu/src/source.c`) — a small residual pop remains. Reverse-engineering the stock firmware
  shows it holds the mute for 500 ms and waits for the new input to report locked; that is the
  fix slated for 0.2.

## 0.2 — in progress (branch `release-0.2`, built from tag `0.1`)

- **Volume: the cap is now stock's full range.** `SETTINGS_VOL_CAP_DEFAULT` 40 -> 71
  (`mcu/src/settings.h`). Up to 0.1 the loudest step was 40 (DSP -21 dB); the original B&W
  firmware allows 71 (+10 dB). So AirPlay at 100 % was 31 dB quieter than stock, and 16-30 dB at
  50-70 %. Applies to the module (AirPlay, Spotify) and USB. The heat protection still lowers it
  to step 50 while the amp is hot. A cap already stored in the EEPROM (`zd1 prefs --volume-cap`)
  overrides the default.
- **Source-switch pop, stock's hold.** The mute hold after a switch goes 80 -> 500 ms
  (`SWITCH_SETTLE_MS`), as in the stock firmware (reverse-engineered: `FUN_8001332c`,
  `FUN_80013a68`). Not done yet: stock also waits for the new input to report locked before
  unmuting; we only hold the time. No deferred switching (that was the broken part of `d284ad3`).
- **The version is in the firmware:** `0.2-<commit>` (`VERSION_ID`, the MCU's stamp), so
  `zd1 info` and the logs show which build is on the speaker.

Not in 0.2: the on-speaker web interface (`zd1-webd`, commit `b550ec0` on `zd1l-show`). It is
reverted from the speaker pending fixes (a real idle state, live polling, real Wi-Fi list / add /
forget, DHCP vs static, Ethernet and Wi-Fi IPs with icons, working restart / factory reset);
planned for 0.3.
