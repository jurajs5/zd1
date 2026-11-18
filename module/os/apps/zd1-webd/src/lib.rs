//! zd1-web: the speaker's web control & settings interface.
//!
//! A small HTTP/1.1 server (std only, one blocking thread per connection) on port 80 of the LAN. It serves one
//! single-page app (`index.html`, built in) and a JSON API that backs the page onto the rest of the system:
//!   - configuration (name, receivers, tone, Wi-Fi/Ethernet, factory reset) through the `zd1-settings` library, which
//!     reads and writes `/data/zd1` the same way `zd1 settings` on the host does; a change is `apply`'d so init
//!     restarts the services that use it;
//!   - live state and control (volume, the source playing, switching source) through `zd1-linkd`'s app socket
//!     (`/run/zd1link`), the same line protocol `zd1-ctl` speaks.
//!
//! Anyone on the local network can reach it: it is the owner's speaker on their own LAN (like the stock web page).
//! It changes nothing a person at the speaker couldn't, and never carries a secret back out (Wi-Fi keys show as
//! "(set)"). Started by init as a system service (module/os/apps/zd1-init, system.rs); off with `zd1.noweb`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;

const PAGE: &str = include_str!("index.html");
const LINK: &str = "/run/zd1link";

pub fn main() {
    let l = TcpListener::bind(("0.0.0.0", 80u16))
        .or_else(|_| TcpListener::bind(("0.0.0.0", 8080u16)))
        .unwrap_or_else(|e| zd1_sys::die("zd1-web", &format!("bind :80: {e}")));
    eprintln!("zd1-web: serving on {}", l.local_addr().map(|a| a.to_string()).unwrap_or_default());
    for s in l.incoming().flatten() {
        std::thread::spawn(move || {
            let _ = serve(s);
        });
    }
}

fn serve(mut s: TcpStream) -> std::io::Result<()> {
    let mut r = BufReader::new(s.try_clone()?);
    let mut req = String::new();
    if r.read_line(&mut req)? == 0 {
        return Ok(());
    }
    let mut it = req.split_whitespace();
    let method = it.next().unwrap_or("").to_string();
    let path = it.next().unwrap_or("/").to_string();

    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; len.min(64 * 1024)];
    r.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body).into_owned();

    let (code, ctype, out) = route(&method, path.split('?').next().unwrap_or(&path), &body);
    let bytes = out.as_bytes();
    write!(
        s,
        "HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        bytes.len()
    )?;
    s.write_all(bytes)?;
    s.flush()
}

fn route(method: &str, path: &str, body: &str) -> (&'static str, &'static str, String) {
    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") => ("200 OK", "text/html; charset=utf-8", PAGE.to_string()),
        ("GET", "/api/status") => ("200 OK", "application/json", api_status()),
        ("GET", "/api/settings") => ("200 OK", "application/json", api_settings()),
        ("POST", "/api/volume") => {
            if let Some(v) = form(body, "value").and_then(|v| v.trim().parse::<u32>().ok()) {
                linkd_send(&format!("volume {}", v.min(100)));
            }
            ok()
        }
        ("POST", "/api/source") => {
            if let Some(id) = form(body, "id").map(|s| sani(&s)).filter(|s| !s.is_empty()) {
                linkd_send(&format!("intent {id}"));
            }
            ok()
        }
        ("POST", "/api/set") => outcome(set_value(form(body, "key"), form(body, "value"))),
        ("POST", "/api/action") => outcome(do_action(form(body, "key"), form(body, "value"))),
        _ => ("404 Not Found", "text/plain; charset=utf-8", "not found".into()),
    }
}

fn set_value(key: Option<String>, value: Option<String>) -> Result<(), String> {
    let key = key.ok_or("no key")?;
    zd1_settings::set(&key, value.as_deref().unwrap_or(""))?;
    zd1_settings::apply(&key);
    Ok(())
}

fn do_action(key: Option<String>, value: Option<String>) -> Result<(), String> {
    let key = key.ok_or("no key")?;
    zd1_settings::act(&key, value.as_deref().unwrap_or(""))?;
    zd1_settings::apply(&key);
    Ok(())
}

fn outcome(r: Result<(), String>) -> (&'static str, &'static str, String) {
    match r {
        Ok(()) => ok(),
        Err(e) => ("400 Bad Request", "application/json", format!("{{\"error\":{}}}", jstr(&e))),
    }
}
fn ok() -> (&'static str, &'static str, String) {
    ("200 OK", "application/json", "{\"ok\":true}".into())
}

/// The live state: the name and firmware, plus whatever zd1-linkd reports (volume, the source, play state, the IPs).
fn api_status() -> String {
    let name = zd1_settings::get("name").unwrap_or_else(zd1_settings::default_name);
    let live = linkd_status();
    format!(
        "{{\"name\":{},\"firmware\":{},\"live\":{}}}",
        jstr(&name),
        jstr(&os_version()),
        if live.is_empty() { "{}".into() } else { live }
    )
}

/// Every setting as it is in effect (secrets shown as "(set)"), and the known Wi-Fi networks.
fn api_settings() -> String {
    let all = zd1_settings::load();
    let fields: Vec<String> = zd1_settings::fields()
        .map(|f| {
            let key = f.key();
            let val = zd1_settings::get_from(&all, &key).unwrap_or_default();
            format!("{}:{}", jstr(&key), jstr(&zd1_settings::shown(&key, &val)))
        })
        .collect();
    let nets: Vec<String> = zd1_settings::networks().iter().map(|n| jstr(&n.ssid)).collect();
    format!("{{\"fields\":{{{}}},\"networks\":[{}]}}", fields.join(","), nets.join(","))
}

/// Ask zd1-linkd for its one-line `status {json}` snapshot (it also streams events; skip those).
fn linkd_status() -> String {
    let go = || -> std::io::Result<String> {
        let s = UnixStream::connect(LINK)?;
        (&s).write_all(b"status\n")?;
        let mut r = BufReader::new(s);
        for _ in 0..8 {
            let mut l = String::new();
            if r.read_line(&mut l)? == 0 {
                break;
            }
            if let Some(json) = l.trim().strip_prefix("status ") {
                return Ok(json.to_string());
            }
        }
        Ok(String::new())
    };
    go().unwrap_or_default()
}

/// A one-line command to zd1-linkd (volume, intent), fire and forget.
fn linkd_send(cmd: &str) {
    if let Ok(mut s) = UnixStream::connect(LINK) {
        let _ = writeln!(s, "{cmd}");
    }
}

fn os_version() -> String {
    std::fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("VERSION_ID="))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_default()
}

/// A value from a urlencoded form body (`key=value&...`).
fn form(body: &str, key: &str) -> Option<String> {
    body.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (urldecode(k) == key).then(|| urldecode(v))
    })
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let h = |c: u8| (c as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (h(b[i + 1]), h(b[i + 2])) {
                    out.push((hi * 16 + lo) as u8);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A source id for the linkd line: lowercase letters and digits only (so nothing can be injected into the protocol).
fn sani(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).take(16).collect()
}

/// A JSON string literal (quotes included).
fn jstr(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
