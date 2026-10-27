// A tiny std-only HTTP server for the local GUI. No async, no dependencies, binds loopback only.
// It serves the embedded UI and a small JSON API that drives the same core the CLI uses; a build is
// run as a child `akt build` with output captured to a log the UI polls.
//
// Security posture (loopback is NOT authorization on its own): every request must carry a Host
// header naming this loopback address (defeats DNS-rebinding, where a browser resolves an attacker
// domain to 127.0.0.1 but still sends the attacker Host), and every state-changing request (POST)
// must have no cross-origin Origin (defeats CSRF from a page the user happens to have open) and
// carry the per-run token embedded in the served page. Reads are size-bounded and enforce an
// ABSOLUTE per-request deadline inside the read loop (a client trickling bytes just under the
// per-read timeout still cannot hold the connection), one build runs at a time behind a cross-process
// flock (held by the build worker, not this server), and the build log is opened without following
// symlinks, under a validated private parent, non-blocking, and only if it is a regular file.
//
// TRUST BOUNDARY (documented, per review): the token is embedded in the page the server returns on
// GET /, so it defends against a browser page the user did not fetch (CSRF/rebinding) and against
// blind same-machine POSTs. It does NOT defend against another local user who can read the served
// page or the process memory: on a shared machine, anyone who can reach the loopback port and read
// the page can obtain the token. The real boundary for local users is OS access control on the host.
use crate::{build, flavor, json_escape, resources, Distro, Plan, FLAVORS};
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const HTML: &str = include_str!("../ui/index.html");
const MAX_HEADER_LINE: usize = 8 * 1024;   // per-line cap on the request line + each header
const MAX_HEADERS: usize = 100;            // cap on header count
const MAX_LOG_BYTES: usize = 256 * 1024;   // cap on the log tail returned to /api/log
const READ_TIMEOUT: Duration = Duration::from_secs(15);     // ceiling on any single blocking read
const REQUEST_DEADLINE: Duration = Duration::from_secs(20); // whole request-header phase
const O_NOFOLLOW: i32 = 0o400000;
const O_NONBLOCK: i32 = 0o4000;

extern "C" { fn getuid() -> u32; }

/// A per-run random token required on state-changing (POST) requests, so a same-machine client that
/// can spoof the Host header still cannot trigger a build without reading the served page. 128 bits
/// of /dev/urandom, hex. If urandom cannot be read we FAIL startup rather than fall back to a
/// guessable pid+time seed (a predictable token would defeat the whole control).
fn gen_token() -> std::io::Result<String> {
    let mut buf = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Constant-time string equality (avoids leaking the token via compare timing).
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for i in 0..a.len() { diff |= a[i] ^ b[i]; }
    diff == 0
}

/// Per-user, non-world-writable log path (avoids the classic /tmp/<fixed-name> symlink hijack).
fn log_path() -> std::path::PathBuf {
    if let Ok(x) = std::env::var("XDG_RUNTIME_DIR") { if !x.is_empty() { return std::path::Path::new(&x).join("akt-build.log"); } }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    std::path::Path::new(&home).join(".cache").join("akt-build.log")
}

/// The log's parent directory must be a real directory we own that is not group- or world-writable,
/// so no other user could have pre-planted a symlink or FIFO there or could swap the file under us.
/// ~/.cache is not guaranteed private, so this is validated, not assumed.
fn validate_private_parent(p: &Path) -> std::io::Result<()> {
    let dir = p.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let md = std::fs::symlink_metadata(dir)?; // symlink_metadata: never follow the parent itself
    if !md.file_type().is_dir() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "log parent is not a directory"));
    }
    if md.uid() != unsafe { getuid() } {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "log parent not owned by current user"));
    }
    if md.mode() & 0o022 != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "log parent is group/world writable"));
    }
    Ok(())
}

/// Verify an already-opened handle is a regular file (defence in depth: even past O_NOFOLLOW, refuse
/// a FIFO/device/socket at the path so the write/read cannot block or be redirected to a special file).
fn assert_regular(f: &std::fs::File) -> std::io::Result<()> {
    if !f.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "log path is not a regular file"));
    }
    Ok(())
}

/// Open the log for a fresh build: create/truncate but NEVER follow a symlink (O_NOFOLLOW), under a
/// validated private parent, non-blocking (a planted FIFO cannot stall the open), and only if the
/// result is a regular file. 0600.
fn open_log() -> std::io::Result<std::fs::File> {
    let p = log_path();
    validate_private_parent(&p)?;
    let _ = std::fs::remove_file(&p);   // remove any prior real file (not through a symlink)
    let f = std::fs::OpenOptions::new().write(true).create(true).truncate(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK).mode(0o600).open(&p)?;
    assert_regular(&f)?;
    Ok(f)
}

/// Read the tail of the build log for /api/log: validate the private parent, open O_NOFOLLOW +
/// O_NONBLOCK (never through a symlink, never blocking on a FIFO), confirm a regular file, and return
/// at most MAX_LOG_BYTES so a symlink-hijacked or huge/streamed log can neither redirect the read nor
/// exhaust memory.
fn read_log_tail() -> std::io::Result<String> {
    let p = log_path();
    validate_private_parent(&p)?;
    let mut f = std::fs::OpenOptions::new().read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK).open(&p)?;
    assert_regular(&f)?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > MAX_LOG_BYTES as u64 {
        use std::io::Seek;
        f.seek(std::io::SeekFrom::End(-(MAX_LOG_BYTES as i64)))?;
    }
    let mut buf = Vec::with_capacity(MAX_LOG_BYTES.min(len as usize));
    (&mut f).take(MAX_LOG_BYTES as u64).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Read the log FROM a byte offset, so a caller can follow the whole build instead of re-fetching a
/// tail. The tail reader above can only ever show the last MAX_LOG_BYTES, which silently truncates a
/// long kernel build: the beginning of the log, where the plan and the resource decisions are
/// printed, had already scrolled out of reach. Returns (chunk, next_offset, total_len). A caller
/// that has fallen more than MAX_LOG_BYTES behind is resynchronised to the tail rather than served a
/// gap it cannot detect.
fn read_log_from(from: u64) -> std::io::Result<(String, u64, u64)> {
    let p = log_path();
    validate_private_parent(&p)?;
    let mut f = std::fs::OpenOptions::new().read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK).open(&p)?;
    assert_regular(&f)?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    // A truncated or rotated log (len < from) restarts the caller from the beginning.
    let mut start = if from > len { 0 } else { from };
    if len.saturating_sub(start) > MAX_LOG_BYTES as u64 {
        start = len - MAX_LOG_BYTES as u64;
    }
    use std::io::Seek;
    f.seek(std::io::SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    (&mut f).take(MAX_LOG_BYTES as u64).read_to_end(&mut buf)?;
    let next = start + buf.len() as u64;
    Ok((String::from_utf8_lossy(&buf).into_owned(), next, len))
}

pub fn serve(port: u16) -> std::io::Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr)?;
    // Fail startup if we cannot mint an unguessable token (rather than serving with a weak one).
    let token = gen_token()?;
    println!("akt GUI on http://{addr}   (Ctrl-C to stop)");
    let _ = Command::new("bash").arg("-c")
        .arg(format!("(xdg-open http://{addr} || sensible-browser http://{addr}) >/dev/null 2>&1 &"))
        .status();
    let mut build: Option<Child> = None;   // the one in-flight build this server spawned, if any
    for stream in listener.incoming() {
        if let Ok(s) = stream { let _ = handle(s, port, &token, &mut build); }
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

/// The outcome of reading one request/header line under the absolute deadline.
enum Line { Data(String), Eof, TooLong, Deadline }

/// Read one line (up to `max` bytes including the newline) with the ABSOLUTE request deadline enforced
/// between reads. Each blocking socket read is bounded to the time remaining until the deadline (capped
/// at READ_TIMEOUT), and the deadline is re-checked every iteration, so a client trickling bytes just
/// under the per-read timeout still cannot hold the connection past the deadline. The socket timeout is
/// only (re)set when the buffered reader must actually touch the socket, keeping normal requests cheap.
fn read_line_deadline(r: &mut BufReader<TcpStream>, s: &TcpStream, deadline: Instant, max: usize)
    -> std::io::Result<Line>
{
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let now = Instant::now();
        if now >= deadline { return Ok(Line::Deadline); }
        if r.buffer().is_empty() {
            s.set_read_timeout(Some((deadline - now).min(READ_TIMEOUT)))?;
        }
        let mut byte = [0u8; 1];
        match r.read(&mut byte) {
            Ok(0) => return Ok(if buf.is_empty() { Line::Eof }
                               else { Line::Data(String::from_utf8_lossy(&buf).into_owned()) }),
            Ok(_) => {
                buf.push(byte[0]);
                if byte[0] == b'\n' { return Ok(Line::Data(String::from_utf8_lossy(&buf).into_owned())); }
                if buf.len() >= max { return Ok(Line::TooLong); }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                return Ok(Line::Deadline),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Decode one application/x-www-form-urlencoded query component.
fn decode_query_component(raw: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }

    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                decoded.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(hi), Some(lo)) => {
                        decoded.push((hi << 4) | lo);
                        i += 3;
                    }
                    _ => {
                        decoded.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                decoded.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn handle(mut s: TcpStream, port: u16, token: &str, build: &mut Option<Child>) -> std::io::Result<()> {
    s.set_read_timeout(Some(READ_TIMEOUT))?;
    s.set_write_timeout(Some(READ_TIMEOUT))?;
    let mut r = BufReader::new(s.try_clone()?);
    let deadline = Instant::now() + REQUEST_DEADLINE;

    // request line (size- and deadline-bounded).
    let line = match read_line_deadline(&mut r, &s, deadline, MAX_HEADER_LINE)? {
        Line::Data(l) => l,
        Line::Eof => return Ok(()),
        Line::TooLong => return respond(&mut s, "431 Request Header Fields Too Large", "text/plain", b"line too long"),
        Line::Deadline => return respond(&mut s, "408 Request Timeout", "text/plain", b"slow request line"),
    };
    if !line.ends_with('\n') { return respond(&mut s, "400 Bad Request", "text/plain", b"malformed request line"); }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    // headers (count-, size-, and deadline-bounded); capture Host + Origin + token; reject duplicate
    // security headers and any header line that overruns the cap or the deadline.
    let (mut host, mut origin, mut req_token) = (String::new(), String::new(), String::new());
    let (mut seen_host, mut seen_origin, mut seen_token) = (false, false, false);
    let mut terminated = false;
    for _ in 0..MAX_HEADERS {
        let h = match read_line_deadline(&mut r, &s, deadline, MAX_HEADER_LINE)? {
            Line::Data(h) => h,
            Line::Eof => break,
            Line::TooLong => return respond(&mut s, "431 Request Header Fields Too Large", "text/plain", b"header too long"),
            Line::Deadline => return respond(&mut s, "408 Request Timeout", "text/plain", b"slow headers"),
        };
        if h == "\r\n" || h == "\n" { terminated = true; break; }
        if !h.ends_with('\n') { return respond(&mut s, "400 Bad Request", "text/plain", b"malformed header"); }
        let low = h.to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("host:") {
            if seen_host { return respond(&mut s, "400 Bad Request", "text/plain", b"duplicate host"); }
            seen_host = true; host = v.trim().to_string();
        } else if let Some(v) = low.strip_prefix("origin:") {
            if seen_origin { return respond(&mut s, "400 Bad Request", "text/plain", b"duplicate origin"); }
            seen_origin = true; origin = v.trim().to_string();
        } else if let Some(v) = low.strip_prefix("x-akt-token:") {
            if seen_token { return respond(&mut s, "400 Bad Request", "text/plain", b"duplicate token"); }
            seen_token = true; req_token = v.trim().to_string();
        }
    }
    if !terminated { return respond(&mut s, "400 Bad Request", "text/plain", b"unterminated headers"); }

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
        query.split('&').find_map(|kv| {
            kv.split_once('=')
                .filter(|(a, _)| *a == k)
                .map(|(_, v)| decode_query_component(v))
        })
    };

    match (method.as_str(), path) {
        ("GET", "/") => {
            // Inject the per-run token so the page (and only a client that fetched it) can POST.
            let page = HTML.replace("__AKT_TOKEN__", token);
            respond(&mut s, "200 OK", "text/html; charset=utf-8", page.as_bytes())
        }
        ("GET", "/api/detect") => {
            let d = Distro::detect();
            json(&mut s, &format!("{{\"distro\":\"{}\",\"supported\":{}}}", d.name(), d != Distro::Unknown))
        }
        ("GET", "/api/system") => {
            let work = crate::build::work_dir(Distro::detect(), "default")
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let info = crate::sysinfo::SystemInfo::detect(&work);
            json(&mut s, &info.to_json())
        }
        ("GET", "/api/plan") => {
            let host = Distro::detect();
            let target = Distro::parse_target(&q("target").unwrap_or_else(|| "host".into()), host).unwrap_or(host);
            let fl = flavor(&q("flavor").unwrap_or_else(|| "default".into())).unwrap_or(&FLAVORS[0]);
            let name = crate::kernel_name(&q("name").unwrap_or_else(|| "arxos".into())).unwrap_or_else(|_| "arxos".into());
            let outq = q("out").filter(|s| !s.is_empty());
            let rel = q("pkgrel").filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).unwrap_or_else(|| "1".into());
            let cpu = q("cpu").unwrap_or_else(|| "auto".into());
            let hz = q("hz").unwrap_or_else(|| "1000".into());
            let thp = q("thp").unwrap_or_else(|| "madvise".into());
            let pgov = q("pgov").as_deref() == Some("1");
            let max_jobs = q("maxjobs").and_then(|v| if v == "auto" { None } else { v.parse::<usize>().ok() }).filter(|v| *v > 0);
            let lto_jobs = q("ltojobs").and_then(|v| if v == "auto" { None } else { v.parse::<usize>().ok() }).filter(|v| *v > 0);
            let source_ref = q("source").filter(|v| !v.is_empty());
            let plan = Plan::resolve(target, fl, &name, outq.as_deref(), &rel)
                .with_tuning(&cpu, &hz, &thp, pgov)
                .with_max_jobs(max_jobs)
                .with_thinlto_jobs(lto_jobs)
                .with_source_ref(source_ref);
            json(&mut s, &plan.to_json())
        }
        ("GET", "/api/explain") => {
            let host = Distro::detect();
            let target = Distro::parse_target(&q("target").unwrap_or_else(|| "host".into()), host).unwrap_or(host);
            let fl = flavor(&q("flavor").unwrap_or_else(|| "default".into())).unwrap_or(&FLAVORS[0]);
            let name = crate::kernel_name(&q("name").unwrap_or_else(|| "arxos".into())).unwrap_or_else(|_| "arxos".into());
            json(&mut s, &crate::explain::as_json(target, fl, &name))
        }
        ("POST", "/api/build") => {
            if !ct_eq(&req_token, token) {
                return respond(&mut s, "403 Forbidden", "text/plain", b"missing or bad build token");
            }
            if !build_free(build) {
                return json(&mut s, "{\"started\":false,\"error\":\"a build is already running\"}");
            }
            let fl = q("flavor").unwrap_or_else(|| "default".into());
            let name = q("name");
            let target = q("target").unwrap_or_else(|| "host".into());
            let cpu = q("cpu").unwrap_or_else(|| "auto".into());
            let hz = q("hz").unwrap_or_else(|| "1000".into());
            let thp = q("thp").unwrap_or_else(|| "madvise".into());
            let pgov = q("pgov").as_deref() == Some("1");
            let max_jobs = q("maxjobs").unwrap_or_else(|| "auto".into());
            let lto_jobs = q("ltojobs").unwrap_or_else(|| "auto".into());
            let source_ref = q("source").filter(|v| !v.is_empty());
            let outq = q("out").filter(|s| !s.is_empty());
            let rel = q("pkgrel").filter(|s| !s.is_empty());
            let dry = q("dry").as_deref() == Some("1");
            // Parity with the CLI. These three were reachable only from a terminal, so a GUI user
            // could not stage a patch, keep a build at the prepare stage, or choose non-strict
            // patching. --patch takes host paths and the CLI is what validates them (it copies the
            // inputs in and refuses symlinks); the server only passes them through.
            let prepare = q("prepare").as_deref() == Some("1");
            let patch_optional = q("patchopt").as_deref() == Some("1");
            let patches: Vec<String> = q("patch")
                .map(|v| v.split('\n').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect())
                .unwrap_or_default();
            let exe = std::env::current_exe().unwrap_or_default();
            match open_log() {
                Ok(out) => {
                    let err = out.try_clone()?;
                    let mut c = Command::new(exe);
                    c.arg("build")
                        .arg("--target-distro").arg(&target)
                        .arg("--cpu-target").arg(&cpu)
                        .arg("--hz").arg(&hz)
                        .arg("--hugepages").arg(&thp)
                        .arg("--max-jobs").arg(&max_jobs)
                        .arg("--thinlto-jobs").arg(&lto_jobs)
                        .arg("--flavor").arg(&fl);
                    if let Some(source) = &source_ref { c.arg("--source-ref").arg(source); }
                    if pgov { c.arg("--performance-governor"); }
                    if let Some(n) = &name { c.arg("--name").arg(n); }
                    if let Some(o) = &outq { c.arg("--out").arg(o); }
                    if let Some(r) = &rel { c.arg("--pkgrel").arg(r); }
                    if dry { c.arg("--dry-run"); }
                    if prepare { c.arg("--prepare-only"); }
                    if patch_optional { c.arg("--patch-optional"); }
                    for pt in &patches { c.arg("--patch").arg(pt); }
                    c.stdout(Stdio::from(out)).stderr(Stdio::from(err));
                    match c.spawn() {
                        Ok(child) => { *build = Some(child); json(&mut s, "{\"started\":true}") }
                        Err(e) => json(&mut s, &format!("{{\"started\":false,\"error\":\"{}\"}}", crate::json_escape(&e.to_string()))),
                    }
                }
                Err(e) => json(&mut s, &format!("{{\"started\":false,\"error\":\"{}\"}}", crate::json_escape(&e.to_string()))),
            }
        }
        // Parity with the CLI: `akt resources`, `akt patches`, `akt gate` and `akt rollback` were
        // terminal-only, so the GUI could not show what the planner will allow, what patches are
        // about to be applied, whether a built config still carries the hardening delta, or roll a
        // kernel back. All four reuse the same code paths the CLI uses.
        ("GET", "/api/resources") => {
            let work = std::env::var("AKT_WORK").unwrap_or_else(|_| ".".into());
            match resources::detect(Path::new(&work)) {
                Ok(r) => json(&mut s, &r.json()),
                Err(e) => json(&mut s, &format!("{{\"error\":\"{}\"}}", json_escape(&e.to_string()))),
            }
        }
        ("GET", "/api/patches") => {
            let fl = q("flavor").unwrap_or_else(|| "default".into());
            let target = q("target").unwrap_or_else(|| "host".into());
            let extra: Vec<String> = q("patch")
                .map(|v| v.split('\n').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect())
                .unwrap_or_default();
            let strict = q("patchopt").as_deref() != Some("1");
            let host = Distro::detect();
            let d = Distro::parse_target(&target, host).unwrap_or(host);
            let flv = flavor(&fl).unwrap_or(&FLAVORS[0]);
            let p = Plan::resolve(d, flv, "arxos", None, "1").with_patches(extra, strict);
            let (files, _dirs) = build::collect_patches(&build::bundled_patch_dirs(p.distro), &p.extra_patches);
            let arr: Vec<String> = files.iter().map(|f| format!("\"{}\"", json_escape(f))).collect();
            json(&mut s, &format!("{{\"patches\":[{}],\"strict\":{}}}", arr.join(","), p.patch_strict))
        }
        ("GET", "/api/gate") => {
            match q("file").filter(|v| !v.is_empty()) {
                None => json(&mut s, "{\"error\":\"no config file given\"}"),
                Some(f) => {
                    let exe = std::env::current_exe().unwrap_or_default();
                    match Command::new(exe).arg("gate").arg(&f).arg("--json").output() {
                        Ok(o) if !o.stdout.is_empty() => {
                            respond(&mut s, "200 OK", "application/json", &o.stdout)
                        }
                        Ok(o) => json(&mut s, &format!("{{\"ok\":{},\"output\":\"{}\"}}",
                            o.status.success(), json_escape(&String::from_utf8_lossy(&o.stderr)))),
                        Err(e) => json(&mut s, &format!("{{\"error\":\"{}\"}}", json_escape(&e.to_string()))),
                    }
                }
            }
        }
        // Rollback reinstalls a package, so it carries the same token requirement as a build and is
        // never reachable by a bare GET.
        ("POST", "/api/rollback") => {
            if !ct_eq(&req_token, token) {
                return respond(&mut s, "403 Forbidden", "text/plain", b"missing or bad build token");
            }
            if !build_free(build) {
                return json(&mut s, "{\"ok\":false,\"error\":\"a build is running\"}");
            }
            let name = q("name").unwrap_or_else(|| "arxos".into());
            let exe = std::env::current_exe().unwrap_or_default();
            match Command::new(exe).arg("rollback").arg("--name").arg(&name).output() {
                Ok(o) => json(&mut s, &format!("{{\"ok\":{},\"output\":\"{}\"}}", o.status.success(),
                    json_escape(&format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))))),
                Err(e) => json(&mut s, &format!("{{\"ok\":false,\"error\":\"{}\"}}", json_escape(&e.to_string()))),
            }
        }
        // Installing writes to the same log the GUI already follows, so the live window shows the
        // install exactly as it shows a build. Token-protected and blocked while a build runs.
        ("POST", "/api/install") => {
            if !ct_eq(&req_token, token) {
                return respond(&mut s, "403 Forbidden", "text/plain", b"missing or bad build token");
            }
            if !build_free(build) {
                return json(&mut s, "{\"started\":false,\"error\":\"a build is running\"}");
            }
            let name = q("name").unwrap_or_else(|| "arxos".into());
            let target = q("target").unwrap_or_else(|| "host".into());
            let keep = q("keep").as_deref() != Some("0");
            let outq = q("out").filter(|v| !v.is_empty());
            let exe = std::env::current_exe().unwrap_or_default();
            match open_log() {
                Ok(out) => {
                    let err = out.try_clone()?;
                    let mut c = Command::new(exe);
                    c.arg("install").arg("--name").arg(&name).arg("--target-distro").arg(&target);
                    if let Some(o) = &outq { c.arg("--out").arg(o); }
                    if !keep { c.arg("--no-keep-running"); }
                    c.stdout(Stdio::from(out)).stderr(Stdio::from(err));
                    match c.spawn() {
                        Ok(child) => { *build = Some(child); json(&mut s, "{\"started\":true}") }
                        Err(e) => json(&mut s, &format!("{{\"started\":false,\"error\":\"{}\"}}", json_escape(&e.to_string()))),
                    }
                }
                Err(e) => json(&mut s, &format!("{{\"started\":false,\"error\":\"{}\"}}", json_escape(&e.to_string()))),
            }
        }
        ("GET", "/api/log") => {
            // With ?from=N the caller follows the log incrementally and sees the WHOLE build; with no
            // offset it keeps the original bounded-tail behaviour so nothing that polled it breaks.
            match q("from").and_then(|v| v.parse::<u64>().ok()) {
                Some(from) => match read_log_from(from) {
                    Ok((chunk, next, total)) => json(&mut s, &format!(
                        "{{\"from\":{},\"next\":{},\"total\":{},\"data\":\"{}\"}}",
                        from, next, total, json_escape(&chunk))),
                    Err(e) => json(&mut s, &format!("{{\"error\":\"{}\"}}", json_escape(&e.to_string()))),
                },
                None => {
                    let body = read_log_tail().unwrap_or_default();
                    respond(&mut s, "200 OK", "text/plain; charset=utf-8", body.as_bytes())
                }
            }
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

#[cfg(test)]
mod query_decode_tests {
    use super::decode_query_component;

    #[test]
    fn decodes_encoded_output_paths() {
        assert_eq!(
            decode_query_component("%2Fhome%2Foxbv1%2FDesktop%2F"),
            "/home/oxbv1/Desktop/"
        );
        assert_eq!(decode_query_component("%2Ftmp%2Fbuild%20dir"), "/tmp/build dir");
    }

    #[test]
    fn handles_plus_and_malformed_escapes() {
        assert_eq!(decode_query_component("hello+world"), "hello world");
        assert_eq!(decode_query_component("%ZZ"), "%ZZ");
    }
}
