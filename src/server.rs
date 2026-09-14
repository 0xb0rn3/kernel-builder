// A tiny std-only HTTP server for the local GUI. No async, no dependencies, binds loopback only.
// It serves the embedded UI and a small JSON API that drives the same core the CLI uses; a build is
// run as a child `akt build` with output captured to a log the UI polls.
//
// Security posture (loopback is NOT authorization on its own): every request must carry a Host
// header naming this loopback address (defeats DNS-rebinding, where a browser resolves an attacker
// domain to 127.0.0.1 but still sends the attacker Host), and every state-changing request (POST)
// must have no cross-origin Origin (defeats CSRF from a page the user happens to have open). Reads
// are time-bounded and size-bounded so a reachable client cannot hang or exhaust the server, one
// build runs at a time behind a lock so concurrent requests cannot erase each other's src/pkg tree,
// and the build log is opened without following symlinks.
use crate::{flavor, Distro, Plan, FLAVORS};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const HTML: &str = include_str!("../ui/index.html");
const MAX_HEADER_LINE: u64 = 8 * 1024;   // per-line cap on the request line + each header
const MAX_HEADERS: usize = 100;          // cap on header count
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// Per-user, non-world-writable log path (avoids the classic /tmp/<fixed-name> symlink hijack).
fn log_path() -> std::path::PathBuf {
    if let Ok(x) = std::env::var("XDG_RUNTIME_DIR") { if !x.is_empty() { return std::path::Path::new(&x).join("akt-build.log"); } }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    std::path::Path::new(&home).join(".cache").join("akt-build.log")
}

/// Open the log for a fresh build: create/truncate but NEVER follow a symlink (O_NOFOLLOW), so a
/// pre-planted symlink at the path cannot redirect the write. 0600.
fn open_log() -> std::io::Result<std::fs::File> {
    let p = log_path();
    if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
    let _ = std::fs::remove_file(&p);   // remove any prior real file (not through a symlink)
    std::fs::OpenOptions::new().write(true).create(true).truncate(true)
        .custom_flags(libc_o_nofollow()).mode(0o600).open(&p)
}
// O_NOFOLLOW without pulling in the libc crate.
fn libc_o_nofollow() -> i32 { 0o400000 }

pub fn serve(port: u16) -> std::io::Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr)?;
    println!("akt GUI on http://{addr}   (Ctrl-C to stop)");
    let _ = Command::new("bash").arg("-c")
        .arg(format!("(xdg-open http://{addr} || sensible-browser http://{addr}) >/dev/null 2>&1 &"))
        .status();
    let mut build: Option<Child> = None;   // the one in-flight build, if any (work lock)
    for stream in listener.incoming() {
        if let Ok(s) = stream { let _ = handle(s, port, &mut build); }
    }
    Ok(())
}

/// True once the tracked build has finished (so a new one may start); reaps it if so.
fn build_free(build: &mut Option<Child>) -> bool {
    match build {
        None => true,
        Some(child) => match child.try_wait() {
            Ok(Some(_)) => { *build = None; true }   // exited -> reap, free
            _ => false,                               // still running (or unknown) -> locked
        },
    }
}

fn handle(mut s: TcpStream, port: u16, build: &mut Option<Child>) -> std::io::Result<()> {
    s.set_read_timeout(Some(READ_TIMEOUT))?;
    s.set_write_timeout(Some(READ_TIMEOUT))?;
    let mut r = BufReader::new(s.try_clone()?);

    // request line (size-bounded)
    let mut line = String::new();
    if (&mut r).take(MAX_HEADER_LINE).read_line(&mut line)? == 0 { return Ok(()); }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    // headers (count- and size-bounded); capture Host + Origin
    let (mut host, mut origin) = (String::new(), String::new());
    for _ in 0..MAX_HEADERS {
        let mut h = String::new();
        if (&mut r).take(MAX_HEADER_LINE).read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" { break; }
        let low = h.to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("host:") { host = v.trim().to_string(); }
        else if let Some(v) = low.strip_prefix("origin:") { origin = v.trim().to_string(); }
    }

    // DNS-rebinding defense: the Host must be this loopback endpoint.
    let ok_host = host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}");
    if !ok_host {
        return respond(&mut s, "421 Misdirected Request", "text/plain", b"bad host");
    }
    // CSRF defense on state change: a cross-origin Origin is rejected (same-origin/none is allowed).
    let is_post = method == "POST";
    if is_post && !origin.is_empty()
        && origin != format!("http://127.0.0.1:{port}") && origin != format!("http://localhost:{port}") {
        return respond(&mut s, "403 Forbidden", "text/plain", b"cross-origin blocked");
    }

    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let q = |k: &str| -> Option<String> {
        query.split('&').find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| v.to_string()))
    };

    match (method.as_str(), path) {
        ("GET", "/") => respond(&mut s, "200 OK", "text/html; charset=utf-8", HTML.as_bytes()),
        ("GET", "/api/detect") => {
            let d = Distro::detect();
            json(&mut s, &format!("{{\"distro\":\"{}\",\"supported\":{}}}", d.name(), d != Distro::Unknown))
        }
        ("GET", "/api/plan") => {
            let fl = flavor(&q("flavor").unwrap_or_else(|| "default".into())).unwrap_or(&FLAVORS[0]);
            let name = crate::kernel_name(&q("name").unwrap_or_else(|| "arxos".into())).unwrap_or_else(|_| "arxos".into());
            let outq = q("out").filter(|s| !s.is_empty());
            let rel = q("pkgrel").filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).unwrap_or_else(|| "1".into());
            json(&mut s, &Plan::resolve(Distro::detect(), fl, &name, outq.as_deref(), &rel).to_json())
        }
        ("GET", "/api/explain") => {
            let fl = flavor(&q("flavor").unwrap_or_else(|| "default".into())).unwrap_or(&FLAVORS[0]);
            let name = crate::kernel_name(&q("name").unwrap_or_else(|| "arxos".into())).unwrap_or_else(|_| "arxos".into());
            json(&mut s, &crate::explain::as_json(Distro::detect(), fl, &name))
        }
        ("POST", "/api/build") => {
            if !build_free(build) {
                return json(&mut s, "{\"started\":false,\"error\":\"a build is already running\"}");
            }
            let fl = q("flavor").unwrap_or_else(|| "default".into());
            let name = q("name");
            let outq = q("out").filter(|s| !s.is_empty());
            let rel = q("pkgrel").filter(|s| !s.is_empty());
            let dry = q("dry").as_deref() == Some("1");
            let exe = std::env::current_exe().unwrap_or_default();
            match open_log() {
                Ok(out) => {
                    let err = out.try_clone()?;
                    let mut c = Command::new(exe);
                    c.arg("build").arg("--flavor").arg(&fl);
                    if let Some(n) = &name { c.arg("--name").arg(n); }
                    if let Some(o) = &outq { c.arg("--out").arg(o); }
                    if let Some(r) = &rel { c.arg("--pkgrel").arg(r); }
                    if dry { c.arg("--dry-run"); }
                    c.stdout(Stdio::from(out)).stderr(Stdio::from(err));
                    match c.spawn() {
                        Ok(child) => { *build = Some(child); json(&mut s, "{\"started\":true}") }
                        Err(e) => json(&mut s, &format!("{{\"started\":false,\"error\":\"{}\"}}", crate::json_escape(&e.to_string()))),
                    }
                }
                Err(e) => json(&mut s, &format!("{{\"started\":false,\"error\":\"{}\"}}", crate::json_escape(&e.to_string()))),
            }
        }
        ("GET", "/api/log") => {
            let body = std::fs::read_to_string(log_path()).unwrap_or_default();
            respond(&mut s, "200 OK", "text/plain; charset=utf-8", body.as_bytes())
        }
        _ => respond(&mut s, "404 Not Found", "text/plain", b"not found"),
    }
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len());
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    s.flush()
}
fn json(s: &mut TcpStream, body: &str) -> std::io::Result<()> { respond(s, "200 OK", "application/json", body.as_bytes()) }
