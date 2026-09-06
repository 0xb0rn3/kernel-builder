// akt — the ARXOS Kernel Tuner.
//
// It applies the CachyOS performance base + the ARXOS hardening / DEATHSTROKE config delta (and, for
// Debian users, Kali-style wireless-injection patches) and builds a tuned kernel on either:
//   * Arch    — fork the CachyOS `linux-cachyos` recipe, overlay the ARXOS config + patches, makepkg
//   * Debian  — take the Debian kernel source, apply CachyOS tunes + the ARXOS config + Kali patches,
//               and build a set of `.deb` packages.
// A local web GUI drives the same core. See README.md.
mod build;
mod explain;
mod gate;
mod server;

use std::path::PathBuf;
use std::process::ExitCode;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Distro { Arch, Debian, Unknown }

impl Distro {
    pub fn detect() -> Distro {
        let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default().to_lowercase();
        let has = |k: &str| os.contains(k);
        if has("arxos") || has("id=arch") || has("cachyos") || has("id_like=arch") || has("arch linux") { Distro::Arch }
        else if has("id=debian") || has("id=ubuntu") || has("id=kali") || has("id_like=debian") { Distro::Debian }
        else { Distro::Unknown }
    }
    pub fn name(&self) -> &'static str {
        match self { Distro::Arch => "arch", Distro::Debian => "debian", Distro::Unknown => "unknown" }
    }
}

/// A flavor selects the scheduler / LTO posture (mirrors linux-arxos build.sh).
#[derive(Clone, Copy)]
pub struct Flavor { pub key: &'static str, pub sched: &'static str, pub lto: &'static str, pub about: &'static str }
pub const FLAVORS: &[Flavor] = &[
    Flavor { key: "default",  sched: "bore",     lto: "thin", about: "balanced daily driver (BORE + ThinLTO)" },
    Flavor { key: "rt",       sched: "rt-bore",  lto: "none", about: "real-time (PREEMPT_RT + BORE; RT and LTO do not mix)" },
    Flavor { key: "hardened", sched: "hardened", lto: "thin", about: "max security posture (KSPP + kCFI)" },
];
pub fn flavor(key: &str) -> Option<&'static Flavor> { FLAVORS.iter().find(|f| f.key == key) }

/// The bundled tune/patch set that lives beside the binary (install) or in the repo (checkout).
pub fn data_dir() -> PathBuf {
    for p in ["/usr/share/kernel-builder", "/usr/share/arxos-kernel-tuner", env!("CARGO_MANIFEST_DIR")] {
        let pb = PathBuf::from(p);
        if pb.join("config/arxos.fragment").exists() { return pb; }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// What a build will do, resolved for the host + flavor. This is the single source of truth the CLI,
/// the GUI, and the build engine all read, so "plan" and "build" never disagree.
pub struct Plan {
    pub distro: Distro,
    pub flavor: &'static Flavor,
    pub kname: String,         // kernel name -> linux-<kname> (default "arxos"; set with --name / --rename)
    pub out: Option<String>,   // output dir for the finished packages (default: the build dir); --out
    pub pkgrel: String,        // package revision / custom version (default "1"); --pkgrel
    pub base: String,          // upstream base we fork/patch
    pub tunes: Vec<String>,    // performance tunes applied
    pub overlays: Vec<String>, // ARXOS config fragments overlaid
    pub patch_sets: Vec<String>,
    pub output: String,        // what artifact we produce
}

impl Plan {
    pub fn resolve(distro: Distro, fl: &'static Flavor, kname: &str, out: Option<&str>, pkgrel: &str) -> Plan {
        let dd = data_dir();
        let arxos_frag = dd.join("config/arxos.fragment").display().to_string();
        match distro {
            Distro::Arch => Plan {
                distro, flavor: fl, kname: kname.to_string(),
                out: out.map(|s| s.to_string()), pkgrel: pkgrel.to_string(),
                base: "CachyOS linux-cachyos (rolling latest)".into(),
                tunes: vec![
                    format!("CachyOS env knobs: _cpusched={}, _use_llvm_lto={}, BBRv3, 1000Hz, x86-64-v3, THP", fl.sched, fl.lto),
                ],
                overlays: vec![arxos_frag],
                patch_sets: vec![dd.join("patches/arxos").display().to_string()],
                output: format!("pacman package  linux-{kname} rev {pkgrel}  (+ headers){}", out.map(|o| format!("  ->  {o}")).unwrap_or_default()),
            },
            Distro::Debian => Plan {
                distro, flavor: fl, kname: kname.to_string(),
                out: out.map(|s| s.to_string()), pkgrel: pkgrel.to_string(),
                base: "Debian kernel source (apt source / linux-source)".into(),
                tunes: vec![
                    "CachyOS tunes ported to a Kconfig fragment: BORE (or EEVDF), BBRv3, 1000Hz, full preempt, MGLRU, THP".into(),
                    format!("scheduler={}, lto={}", fl.sched, fl.lto),
                ],
                overlays: vec![arxos_frag],
                patch_sets: vec![
                    dd.join("patches/arxos").display().to_string(),
                    dd.join("patches/kali").display().to_string(),
                ],
                output: format!("Debian packages  linux-image-*-{kname}.deb  +  linux-headers-*.deb{}", out.map(|o| format!("  ->  {o}")).unwrap_or_default()),
            },
            Distro::Unknown => Plan {
                distro, flavor: fl, kname: kname.to_string(),
                out: out.map(|s| s.to_string()), pkgrel: pkgrel.to_string(),
                base: "unknown host".into(), tunes: vec![], overlays: vec![], patch_sets: vec![],
                output: "unsupported — akt supports Arch-family and Debian-family hosts".into(),
            },
        }
    }
    pub fn to_json(&self) -> String {
        let arr = |v: &[String]| {
            let items: Vec<String> = v.iter().map(|s| format!("\"{}\"", json_escape(s))).collect();
            format!("[{}]", items.join(","))
        };
        format!(
            "{{\"distro\":\"{}\",\"flavor\":\"{}\",\"kernel\":\"{}\",\"pkgrel\":\"{}\",\"out\":\"{}\",\"base\":\"{}\",\"tunes\":{},\"overlays\":{},\"patch_sets\":{},\"output\":\"{}\"}}",
            self.distro.name(), self.flavor.key, json_escape(&self.kname), json_escape(&self.pkgrel), json_escape(self.out.as_deref().unwrap_or("")), json_escape(&self.base),
            arr(&self.tunes), arr(&self.overlays), arr(&self.patch_sets), json_escape(&self.output))
    }
}

pub fn json_escape(s: &str) -> String { s.replace('\\', "\\\\").replace('"', "\\\"") }

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}
fn has_flag(args: &[String], name: &str) -> bool { args.iter().any(|a| a == name) }

/// Validate + normalize the kernel name (becomes the pkgsuffix + LOCALVERSION + package filenames).
fn kernel_name(s: &str) -> Result<String, String> {
    let n = s.trim().to_lowercase();
    if n.is_empty() || n.len() > 24 { return Err(format!("--name '{s}': must be 1-24 characters")); }
    let ok = n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !n.starts_with('-') && !n.ends_with('-');
    if !ok { return Err(format!("--name '{s}': lowercase letters, digits and dashes only, no leading/trailing dash")); }
    Ok(n)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    let json = has_flag(&args, "--json");
    let fl = flavor(flag(&args, "--flavor").unwrap_or("default")).unwrap_or(&FLAVORS[0]);
    let kname = match kernel_name(flag(&args, "--name").or_else(|| flag(&args, "--rename")).unwrap_or("arxos")) {
        Ok(n) => n,
        Err(e) => { eprintln!("akt: {e}"); return ExitCode::FAILURE; }
    };
    let out = flag(&args, "--out").map(|s| s.to_string());
    let pkgrel = flag(&args, "--pkgrel").unwrap_or("1").to_string();
    if pkgrel.is_empty() || !pkgrel.chars().all(|c| c.is_ascii_digit()) {
        eprintln!("akt: --pkgrel must be a number (the package revision / version, default 1)"); return ExitCode::FAILURE;
    }

    match cmd {
        "detect" => {
            let d = Distro::detect();
            if json { println!("{{\"distro\":\"{}\",\"supported\":{}}}", d.name(), d != Distro::Unknown); }
            else {
                println!("host distro : {}", d.name());
                println!("build path  : {}", match d {
                    Distro::Arch => "Arch — fork CachyOS + overlay ARXOS config/patches -> pacman package",
                    Distro::Debian => "Debian — Debian source + CachyOS tunes + ARXOS + Kali patches -> .deb",
                    Distro::Unknown => "unsupported (need an Arch-family or Debian-family host)",
                });
            }
            ExitCode::SUCCESS
        }
        "plan" => {
            let p = Plan::resolve(Distro::detect(), fl, &kname, out.as_deref(), &pkgrel);
            if json { println!("{}", p.to_json()); }
            else {
                println!("== akt tune plan ({}, flavor {}) ==", p.distro.name(), p.flavor.key);
                println!("  base     : {}", p.base);
                for t in &p.tunes { println!("  tune     : {t}"); }
                for o in &p.overlays { println!("  overlay  : {o}"); }
                for ps in &p.patch_sets { println!("  patches  : {ps}"); }
                println!("  output   : {}", p.output);
            }
            ExitCode::SUCCESS
        }
        "explain" => {
            explain::print(Distro::detect(), fl, &kname, json);
            ExitCode::SUCCESS
        }
        "build" => {
            let dry = has_flag(&args, "--dry-run");
            match build::run(Plan::resolve(Distro::detect(), fl, &kname, out.as_deref(), &pkgrel), dry) {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => { eprintln!("akt: build failed: {e}"); ExitCode::FAILURE }
            }
        }
        "gate" => {
            let cfg = args.get(1).filter(|a| !a.starts_with("--"));
            match cfg {
                Some(path) => if gate::verify(path, json) { ExitCode::SUCCESS } else { ExitCode::FAILURE },
                None => { eprintln!("akt gate <config-file>"); ExitCode::FAILURE }
            }
        }
        "gui" => {
            let port: u16 = flag(&args, "--port").and_then(|p| p.parse().ok()).unwrap_or(8787);
            match server::serve(port) { Ok(_) => ExitCode::SUCCESS, Err(e) => { eprintln!("akt gui: {e}"); ExitCode::FAILURE } }
        }
        "version" | "--version" | "-V" => { println!("akt {VERSION}"); ExitCode::SUCCESS }
        "" | "help" | "--help" | "-h" => { println!("{}", usage()); ExitCode::SUCCESS }
        other => { eprintln!("akt: unknown command '{other}'\n\n{}", usage()); ExitCode::FAILURE }
    }
}

fn usage() -> String {
    format!(
"akt {VERSION} — ARXOS Kernel Tuner

  detect                 show the host distro + which build path applies
  plan   [--flavor F]    print the tune plan (base, tunes, patches) without building
  build  [--flavor F] [--name NAME] [--pkgrel N] [--out DIR] [--dry-run]
                         build a tuned kernel for this host
                         --name / --rename  builds linux-<NAME> (default: arxos)
                         --pkgrel N         package revision / custom version (default: 1)
                         --out DIR          write the finished packages to DIR
  explain [--flavor F] [--name NAME]
                         explain what each option means for your custom kernel
  gate   <config-file>   verify the ARXOS hardening delta is present in a built .config
  gui    [--port N]      launch the local web GUI (default http://127.0.0.1:8787)
  version

FLAVORS: default (BORE+ThinLTO) | rt (PREEMPT_RT) | hardened (KSPP+kCFI)
Add --json to detect/plan/gate for machine-readable output.")
}
