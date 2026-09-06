// A tiny std-only HTTP server for the local GUI. No async, no dependencies, binds loopback only.
// It serves the embedded UI and a small JSON API that drives the same core the CLI uses; a build is
// run as a child `akt build` with output captured to a log the UI polls.
use crate::{flavor, Distro, Plan, FLAVORS};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};

const HTML: &str = include_str!("../ui/index.html");
const LOG: &str = "/tmp/akt-build.log";

pub fn serve(port: u16) -> std::io::Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr)?;
    println!("akt GUI on http://{addr}   (Ctrl-C to stop)");
    let _ = Command::new("bash").arg("-c")
        .arg(format!("(xdg-open http://{addr} || sensible-browser http://{addr}) >/dev/null 2>&1 &"))
        .status();
    for stream in listener.incoming() {
        if let Ok(s) = stream { let _ = handle(s); }
    }
    Ok(())
}

fn handle(mut s: TcpStream) -> std::io::Result<()> {
    let mut r = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 { return Ok(()); }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    loop { let mut h = String::new(); if r.read_line(&mut h)? == 0 || h == "\r\n" { break; } }

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
            let fl = q("flavor").unwrap_or_else(|| "default".into());
            let name = q("name");
            let outq = q("out").filter(|s| !s.is_empty());
            let rel = q("pkgrel").filter(|s| !s.is_empty());
            let dry = q("dry").as_deref() == Some("1");
            let exe = std::env::current_exe().unwrap_or_default();
            match std::fs::File::create(LOG) {
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
                        Ok(_) => json(&mut s, "{\"started\":true}"),
                        Err(e) => json(&mut s, &format!("{{\"started\":false,\"error\":\"{}\"}}", crate::json_escape(&e.to_string()))),
                    }
                }
                Err(e) => json(&mut s, &format!("{{\"started\":false,\"error\":\"{}\"}}", crate::json_escape(&e.to_string()))),
            }
        }
        ("GET", "/api/log") => {
            let body = std::fs::read_to_string(LOG).unwrap_or_default();
            respond(&mut s, "200 OK", "text/plain; charset=utf-8", body.as_bytes())
        }
        _ => respond(&mut s, "404 Not Found", "text/plain", b"not found"),
    }
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len());
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    s.flush()
}
fn json(s: &mut TcpStream, body: &str) -> std::io::Result<()> { respond(s, "200 OK", "application/json", body.as_bytes()) }
