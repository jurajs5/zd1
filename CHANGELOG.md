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

## 0.2 — next build (in progress)

Planned:

- **Source-switch pop — the proper fix,** from reverse-engineering the stock B&W firmware
  (`FUN_80013a68` and friends): hold the amp and DAC muted 500 ms after the input switch
  (`SWITCH_SETTLE_MS` 80 -> 500) and gate the unmute on the new input reporting locked
  (S/PDIF lock for AUX), with a timeout. No deferred switching.
- **On-speaker web control & settings interface** (`zd1-webd`): an HTTP server on the speaker
  (port 80) serving a control & settings page. Built on branch at commit `b550ec0`, then
  **reverted off the speaker** pending fixes: real "nothing playing" state, live polling so the
  UI follows the speaker, real Wi-Fi list / add (with PSK) / forget, DHCP vs static IP done
  right, a thinner light-show strip, both Ethernet and Wi-Fi IPs shown with icons, and working
  restart / factory reset.
