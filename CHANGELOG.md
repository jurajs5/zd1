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

## 0.4 — the speaker's control panel (web), on port 80

A page served by the speaker itself (`zd1-webd`, `module/os/apps/zd1-webd`), reachable by IP
(`http://<speaker>/`), plain HTTP and no login, so for a trusted home network only. A Wi-Fi
passphrase typed there crosses the LAN unencrypted (the speaker stores only the derived key).

- **Control:** nothing-playing state (no made-up track), the receiver in front and its play state,
  volume (live), receivers listed with their state (AirPlay, Spotify Connect, Snapcast); a tap
  asks the speaker to switch to that receiver. The page polls every 2 s, so it follows changes made on
  the speaker or in an app. A thin light-show strip animates while something plays (a preview only).
- **Settings:** name; Ethernet and Wi-Fi each with their address shown (header: both, with icons),
  DHCP (the address is read-only) or a static IP (address/prefix, gateway, DNS, then Save); known
  Wi-Fi networks with Forget; add a network (the WPA2 key is derived on the speaker, PBKDF2-SHA1);
  receivers on/off — **Spotify Connect can be switched off here**; firmware; Restart; Reset settings
  (keeps saved networks and AirPlay pairings). Language (English, Čeština) and theme (system, light, dark).
- Not in it, because the MCU would have to support them first and nothing in the panel is a mock any more:
  bass, LED colours, the light show on/off, transport keys, USB/AUX as inputs (the module only knows its receivers).
- `zd1-linkd` answers a new `status` line (volume, receiver in front, play state, IPs, link, Wi-Fi network and signal).
- Includes 0.3.1 (AirPlay no longer stays silent after a failed Spotify start).

## 0.3.1 — AirPlay's play in the same session also drops a stale yield

0.3's fix only ran on a fresh pick (`intent`). Tested on the unit: after a Spotify pick (refused key)
the iPhone paused AirPlay (our `yield` pause) and its next play started a new stream in the SAME
session, with no new `session set up` and so no `intent`: still silent (`Audio session initialized`,
no `speaker state="playing"`). `zd1-airplay` now says `intent` when a stream starts while the
receiver is `yielded`, which brings back the `yield` to itself that 0.3 already handles.
Spotify with the 0.3 device id and on another VLAN: the audio key is still refused (`0 1`).

## 0.3 — AirPlay no longer stays silent after a failed Spotify; a new Spotify device id

Module only (`mcu/` is 0.2.1's: switch hold 80 ms, cap default 71).

- **AirPlay stuck silent after Spotify.** Picking a receiver makes `zd1-linkd` broadcast `yield X`
  to every app, X included, and only `select` or the old owner's `resume` cleared a receiver's
  `yielded`. After a Spotify pick that never played (audio key refused: no owner, so no `resume`),
  AirPlay stayed `yielded`: it received its stream and dropped it, never said `playing`, and the MCU
  saw `paused`. `zd1-audio` now treats its own `yield` as "the speaker is ours" and drops a stale
  yield (`module/os/apps/zd1-audio/src/speaker.rs`). Before: only `zd1-ctl restart airplay` helped.
- **New Spotify device id** (`zd1-spotify`): a 40-hex id derived from the MAC and a salt, instead of
  the MAC padded with zeros. To Spotify the speaker is a new device. An experiment: the audio-key
  refusal is reported per account (librespot-org/librespot#1649), so it may not help.

## 0.2.1 — switch speed back to 0.1's

- `SWITCH_SETTLE_MS` 500 -> 80 ms: with stock's 500 ms hold every input switch felt slow, so the
  hold is back to what 0.1 had. A small switch pop may return; the stock lock gate is the planned
  proper fix. Everything else is 0.2.

## 0.2 — in progress (branch `release-0.2`, built from tag `0.1`)

- **Volume: the cap is now stock's full range.** `SETTINGS_VOL_CAP_DEFAULT` 40 -> 71
  (`mcu/src/settings.h`). Up to 0.1 the loudest step was 40 (DSP -21 dB); the original B&W
  firmware allows 71 (+10 dB). So AirPlay at 100 % was 31 dB quieter than stock, and 16-30 dB at
  50-70 %. Applies to the module (AirPlay, Spotify) and USB. The heat protection still lowers it
  to step 50 while the amp is hot. **A cap already stored in the EEPROM overrides the default**
  (`zd1 prefs --volume-cap`): on the first unit it was still 40 after flashing 0.2, so it had to be
  set once with `zd1 prefs --volume-cap 71` (needs the ON/OFF unlock). Check `zd1 prefs`.
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

### Found while testing 0.2 (not fixed yet; planned for 0.3)

- **AirPlay silent after a failed Spotify start.** If Spotify Connect takes the speaker and then fails
  (here: `Spotify refused its audio key`), `zd1-linkd` keeps Spotify as the owner and `zd1-audio` keeps
  AirPlay in `yielded`, so AirPlay receives and drops its packets and never reports `playing`; the MCU
  only sees `paused`. Restarting the `airplay` service clears it. Fix: release the owner when the
  session ends or the player never starts, and clear `yielded` when the owner goes.
- **Spotify Connect does not play** on this unit now: `audio key refused` (server side, cause unknown).
- **Wi-Fi loop in `wifi.log`** with no network saved: wpa_supplicant keeps associating to the last AP and
  disconnecting. Harmless so far.
