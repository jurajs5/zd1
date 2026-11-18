//! zd1-airplay: the module's AirPlay 1/2 receiver, on shairplay-rust (our fork, module/os/forks.md; built with the integer
//! audio path: our fixed-point AAC-LC decoder (module/codecs/aac), integer samples end to end at the source's depth
//! (16-bit; 24-bit for lossless ALAC/48000/S24, which keeps its depth to the DSP), no resampler;
//! docs/streaming.md). Plays into ALSA (zd1-audio's output: snd-sulu, AV port 2 -> the speaker's DSP) and reports
//! to the speaker through zd1-linkd (zd1-audio's speaker link): play state, the sender's volume (the speaker sets the
//! level; samples go out at full scale), its keys and volume -> the sender; a sender starts from the speaker's
//! volume (it asks: `AudioHandler::volume`).
//!
//!   zd1-airplay [--name NAME] [--device hw:0,0] [--pin PIN] [--buffer-kb KB] [--store DIR] [--no-warp0] [-v]
//!
//! Name: "Zeppelin Air XXXXXX" from eth0's MAC, like the stock module. Pairing: transient (no PIN, as AirPlay
//! speakers without HomeKit) unless --pin; the accessory's identity and PIN-paired controllers are kept in
//! /data/zd1/airplay (`store`), so the speaker stays the same accessory across restarts.
//! Warp0 does the decoding when it is free, unless --no-warp0 (module/warp0: the ARM only moves packets and PCM):
//! realtime ALAC streams (AirPlay 1, AirPlay 2 realtime) decrypted and decoded per packet, buffered AAC streams
//! (an iPhone's music) and buffered ALAC ones (an Apple TV's lossless, 16- or 24-bit) decoded frame by frame ahead of
//! playout; anything it can't take runs on the ARM.

use shairplay::{
    AudioFormat, AudioHandler, AudioSession, DeviceIdentity, RaopServer, RemoteCommand, RemoteControl, SampleFormat,
    Samples, TrackMetadata,
};
use std::sync::{Arc, Mutex};
use tokio::signal::unix::{SignalKind, signal};
use zd1_audio::{Args, Output, Stream, speaker};

mod store;

pub const SOURCE: &str = "airplay"; // our name to zd1-linkd (`state playing airplay`, `yield <other>`)

/// What the server callbacks and the speaker link share.
pub struct Shared {
    pub remote: Option<Arc<dyn RemoteControl>>, // the current sender's remote control (DACP / AP2 commands)
    pub connections: usize,                     // open sender connections (an iPhone keeps a remote-control one
    // beside the audio one and closes it on its own)
    pub sessions: usize, // sessions with this speaker as the output (not remote-control-only)
}

/// The speaker's keys, yields and volume -> the sender's remote control
fn on_speaker(shared: &Arc<Mutex<Shared>>, ev: speaker::Event) {
    let (cmd, why) = match ev {
        speaker::Event::Key { key, .. } => (
            match key {
                "play" => RemoteCommand::Play,
                "pause" => RemoteCommand::Pause,
                // the sender toggles: we can't always tell (a Mac's system audio streams on for ~14 s after a pause)
                "playpause" => RemoteCommand::PlayPause,
                "next" => RemoteCommand::NextTrack,
                "prev" => RemoteCommand::PreviousTrack,
                _ => return,
            },
            key,
        ),
        // another receiver or source took the speaker: pause the sender, if it plays here (one that doesn't may be a
        // Mac whose now-playing app plays elsewhere: the pause would reach it)
        speaker::Event::Yield { playing: false } | speaker::Event::Release { playing: false } => return,
        speaker::Event::Yield { .. } => (RemoteCommand::Pause, "yield"),
        speaker::Event::Release { .. } => (RemoteCommand::Pause, "released"),
        speaker::Event::Volume(v) => (RemoteCommand::SetVolume(v.min(100) as u8), "volume"), // its slider follows
    };
    let r = shared.lock().unwrap().remote.clone();
    match r {
        Some(r) => match r.send_command(cmd.clone()) {
            Ok(()) => tracing::info!(?cmd, why, "-> sender"),
            Err(e) => tracing::warn!(?cmd, why, %e, "sender refused"),
        },
        None => tracing::info!(why, "no sender to control"),
    }
}

/// What plays, for the web panel (zd1-webd): `/run/zd1/now.json` (a small JSON object: title, artist, album and the
/// cover's address when there is one) and `/run/zd1/cover` (the picture as the sender gave it). RAM only (/run). A sender
/// that sends none leaves the panel with the receiver's name; the files go when the session ends.
mod now {
    use super::TrackMetadata;
    use std::sync::Mutex;
    const DIR: &str = "/run/zd1";
    const MAX_COVER: usize = 400 * 1024;
    #[derive(Default)]
    struct Now {
        title: String,
        artist: String,
        album: String,
        cover: u32, // 0: none; else a counter, so the page loads a new picture
    }
    static NOW: Mutex<Option<Now>> = Mutex::new(None);

    fn json(s: &str) -> String {
        let mut o = String::with_capacity(s.len() + 2);
        o.push('"');
        for c in s.chars() {
            match c {
                '"' => o.push_str("\\\""),
                '\\' => o.push_str("\\\\"),
                c if (c as u32) < 0x20 => o.push(' '),
                c => o.push(c),
            }
        }
        o.push('"');
        o
    }
    fn write(n: &Now) {
        let _ = std::fs::create_dir_all(DIR);
        let cover = if n.cover > 0 { format!("\"/api/cover?v={}\"", n.cover) } else { "\"\"".into() };
        let body = format!(
            "{{\"title\":{},\"artist\":{},\"album\":{},\"cover\":{cover}}}",
            json(&n.title),
            json(&n.artist),
            json(&n.album)
        );
        let tmp = format!("{DIR}/now.json.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, format!("{DIR}/now.json"));
        }
    }
    pub fn metadata(m: &TrackMetadata) {
        let mut g = NOW.lock().unwrap();
        let n = g.get_or_insert_with(Now::default);
        n.title = m.title.clone().unwrap_or_default();
        n.artist = m.artist.clone().unwrap_or_default();
        n.album = m.album.clone().unwrap_or_default();
        write(n);
    }
    pub fn cover(bytes: &[u8]) {
        if bytes.is_empty() || bytes.len() > MAX_COVER {
            return;
        }
        let _ = std::fs::create_dir_all(DIR);
        let tmp = format!("{DIR}/cover.tmp");
        if std::fs::write(&tmp, bytes).is_err() || std::fs::rename(&tmp, format!("{DIR}/cover")).is_err() {
            return;
        }
        let mut g = NOW.lock().unwrap();
        let n = g.get_or_insert_with(Now::default);
        n.cover = n.cover.wrapping_add(1).max(1);
        write(n);
    }
    pub fn clear() {
        *NOW.lock().unwrap() = None;
        let _ = std::fs::remove_file(format!("{DIR}/now.json"));
        let _ = std::fs::remove_file(format!("{DIR}/cover"));
    }
}

struct Handler {
    out: Output,
    shared: Arc<Mutex<Shared>>,
    speaker: speaker::Speaker,
}

impl AudioHandler for Handler {
    fn audio_init(&self, format: AudioFormat) -> Box<dyn AudioSession> {
        let bits = if format.format == SampleFormat::S32 { 32 } else { 16 };
        tracing::info!(rate = format.sample_rate, channels = format.channels, bits, "stream");
        if self.speaker.yielded() {
            // a sender that starts (again) to play while we are still yielded to another source (one that was picked
            // and never played: Spotify's refused key leaves no `resume`, and the sender's play in the same session
            // is no new `session set up`) is a person choosing this speaker: say so, and zd1-linkd's `yield` back to
            // us drops the stale yield (zd1-audio's speaker.rs)
            tracing::info!("a stream starts while yielded: picked again");
            self.speaker.intent();
        }
        let stream = self.out.start_stream(format.sample_rate, format.channels, bits);
        Box::new(Session { out: self.out.clone(), stream })
    }

    // AirPlay volume: -30.0 .. 0.0 dB, -144 = mute -> the speaker's 0-100 (as shairport-sync's hook did)
    fn on_volume(&self, db: f32) {
        let v = percent_from_db(db);
        tracing::info!(db, speaker = v, "sender's volume");
        self.speaker.send(&format!("volume {v}"));
    }

    // what a sender starts from: the speaker's own level (else it shows full volume and sets it); not known yet
    // after a boot (the MCU tells its volume only when it changes): a low one
    fn volume(&self) -> Option<f32> {
        let level = self.speaker.level();
        let db = level.map_or(UNKNOWN_DB, db_from_percent);
        tracing::info!(?level, db, "volume asked");
        Some(db)
    }

    fn on_remote_control(&self, remote: Arc<dyn RemoteControl>) {
        self.shared.lock().unwrap().remote = Some(remote);
    }

    fn on_client_connected(&self, addr: &str) {
        self.shared.lock().unwrap().connections += 1;
        tracing::info!(addr, "sender connected");
    }

    fn on_session_setup(&self, addr: &str) {
        tracing::info!(addr, "session set up: the speaker was picked");
        self.shared.lock().unwrap().sessions += 1;
        self.speaker.session(true); // a sender with the speaker as its output (not a remote's look)
        self.speaker.intent();
    }

    fn on_metadata(&self, metadata: &TrackMetadata) {
        now::metadata(metadata);
    }

    fn on_coverart(&self, coverart: &[u8]) {
        now::cover(coverart);
    }

    fn on_session_end(&self, addr: &str) {
        let left = {
            let mut s = self.shared.lock().unwrap();
            s.sessions = s.sessions.saturating_sub(1);
            s.sessions
        };
        tracing::info!(addr, left, "session ended");
        if left == 0 {
            self.speaker.session(false);
            now::clear();
        } // its remote-control connection may stay: not a sender
    }

    fn on_client_disconnected(&self, addr: &str) {
        let mut shared = self.shared.lock().unwrap();
        shared.connections = shared.connections.saturating_sub(1);
        tracing::info!(addr, left = shared.connections, "sender connection closed");
        if shared.connections == 0 {
            shared.remote = None;
            drop(shared);
            self.out.stop();
        }
    }
}

/// What a sender is told before the speaker's volume is known: shairport-sync's default (-24 dB: 20 %, step 14)
const UNKNOWN_DB: f32 = -24.0;

fn percent_from_db(db: f32) -> u32 {
    if db <= -30.0 { 0 } else { (((db + 30.0) * 100.0 / 30.0 + 0.5) as u32).min(100) }
}

fn db_from_percent(p: u32) -> f32 {
    if p == 0 { -144.0 } else { p.min(100) as f32 * 30.0 / 100.0 - 30.0 }
}

/// One stream's audio into the output; a newer stream (another sender took over) makes it push into nothing.
struct Session {
    out: Output,
    stream: Stream,
}

impl AudioSession for Session {
    fn audio(&mut self, samples: Samples<'_>, play_at_ns: Option<u64>) {
        match samples {
            Samples::S16(s) => self.out.push(self.stream, s, play_at_ns),
            Samples::S32(s) => self.out.push(self.stream, s, play_at_ns),
            Samples::F32(_) => {} // never: the server is built for integer samples
        }
    }
    fn audio_flush(&mut self) {
        self.out.flush(self.stream);
    }
}

/// How the speaker presents itself to senders: as what it is, a third-party speaker (its own model and maker, no
/// Apple OS version), as certified ones do; senders then show a speaker, not an Apple TV.
fn identity(mac: &[u8]) -> DeviceIdentity {
    let release = zd1_sys::OsRelease::read("/etc/os-release");
    let version = release.get("VERSION_ID");
    DeviceIdentity {
        model: "Zeppelin Air".into(),
        manufacturer: Some("Bowers & Wilkins".into()),
        serial_number: Some(mac.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("-")),
        firmware_version: format!("zd1-os {version}"),
        os_version: None,
    }
}

const USAGE: &str =
    "zd1-airplay [--name NAME] [--device hw:0,0] [--pin PIN] [--buffer-kb KB] [--store DIR] [--no-warp0] [-v]";

#[tokio::main(flavor = "current_thread")]
pub async fn main() {
    zd1_compat::init();
    let args = Args::parse(USAGE, &["--name", "--device", "--pin", "--buffer-kb", "--store"], &["--no-warp0", "-v"]);
    args.log();
    let mac = zd1_audio::app::mac().to_vec();
    let name = args.name();
    // how far ahead an AirPlay 2 sender may stream, in compressed bytes: what the playout queue holds (4 MB, ~20 s
    // of ALAC/48000/S24). An Apple TV paces lossless by it: with 256 KB it sent a second every 10 s
    let buffer_kb: u32 = args.get_or("--buffer-kb", 4096);
    let warp0 = !args.flag("--no-warp0");

    let shared = Arc::new(Mutex::new(Shared { remote: None, connections: 0, sessions: 0 }));
    let s2 = shared.clone();
    let speaker = speaker::Speaker::start(SOURCE, move |ev| on_speaker(&s2, ev));
    let out = Output::start(args.device(), speaker.clone());
    let handler = Arc::new(Handler { out, shared, speaker });

    let mut builder = RaopServer::builder()
        .name(name.clone())
        .identity(identity(&mac))
        .hwaddr(mac)
        .sample_format(SampleFormat::Integer)
        .audio_buffer_size(buffer_kb * 1024)
        .decoders(zd1_warp0::decoders(warp0))
        .pairing_store(Arc::new(store::DataStore::open(std::path::Path::new(
            args.get("--store").unwrap_or("/data/zd1/airplay"),
        ))))
        .max_clients(4);
    if let Some(pin) = args.get("--pin") {
        builder = builder.pin(pin.to_string());
    }
    builder = lab_options(builder);
    let mut server = match builder.build(handler) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("zd1-airplay: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = server.start().await {
        eprintln!("zd1-airplay: {e}");
        std::process::exit(1);
    }
    tracing::info!(name, "receiver up");
    // init stops and restarts us with SIGTERM (a setting changed: the PIN, the name): the stop's mDNS goodbyes tell
    // the senders' caches that the old records are gone, so none keeps asking for a PIN that no longer exists
    let mut term = signal(SignalKind::terminate()).expect("a SIGTERM handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    tracing::info!("stopping");
    server.stop().await;
}

/// The measurement knobs (docs/streaming.md), from the environment as they always were: the clock samples to a
/// CSV (SHAIRPLAY_PTP_LOG=FILE), no Delay_Req (SHAIRPLAY_NO_DELAY_REQ), no PTP sink (SHAIRPLAY_NO_PTP), the
/// buffered offset and the realtime latency in ms (SHAIRPLAY_BUFFERED_LATENCY_MS, SHAIRPLAY_REALTIME_LATENCY_MS).
fn lab_options(mut b: shairplay::RaopServerBuilder) -> shairplay::RaopServerBuilder {
    let var = |k: &str| std::env::var(k).ok();
    let ms = |k: &str| var(k).and_then(|v| v.parse::<f64>().ok());
    if let Some(path) = var("SHAIRPLAY_PTP_LOG") {
        b = b.ptp_log(path);
    }
    if var("SHAIRPLAY_NO_DELAY_REQ").is_some() {
        b = b.ptp_delay_req(false);
    }
    if var("SHAIRPLAY_NO_PTP").is_some() {
        b = b.ptp(false);
    }
    if let Some(v) = ms("SHAIRPLAY_BUFFERED_LATENCY_MS") {
        b = b.buffered_offset_ms(v);
    }
    if let Some(v) = ms("SHAIRPLAY_REALTIME_LATENCY_MS") {
        b = b.realtime_latency_ms(v);
    }
    b
}
