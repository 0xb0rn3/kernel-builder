// akt — the ARXOS Kernel Tuner.
//
// It applies the upstream performance base + the ARXOS hardening config delta (and, for
// Debian users, Kali-style wireless-injection patches) and builds a tuned kernel on either:
//   * Arch    - fork the upstream performance recipe, overlay the ARXOS config + patches, makepkg
//   * Debian  - take the Debian kernel source, apply the upstream tunes + the ARXOS config + Kali patches,
//               and build a set of `.deb` packages.
// A local web GUI drives the same core. See README.md.
mod build;
mod explain;
mod gate;
mod install;
mod server;
mod resources;
mod sysinfo;
mod container_build;

use std::path::PathBuf;
use std::process::ExitCode;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Distro { Arch, Debian, Unknown }

impl Distro {
    pub fn detect() -> Distro {
        let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let mut id = String::new();
        let mut like = String::new();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("ID=") { id = v.trim_matches('"').to_ascii_lowercase(); }
            if let Some(v) = line.strip_prefix("ID_LIKE=") { like = v.trim_matches('"').to_ascii_lowercase(); }
        }
        let ids: Vec<&str> = std::iter::once(id.as_str()).chain(like.split_whitespace()).collect();
        if ids.iter().any(|v| matches!(*v, "arxos" | "arch" | "cachyos")) { Distro::Arch }
        else if ids.iter().any(|v| matches!(*v, "debian" | "ubuntu" | "kali")) { Distro::Debian }
        else { Distro::Unknown }
    }
    pub fn name(&self) -> &'static str {
        match self { Distro::Arch => "arch", Distro::Debian => "debian", Distro::Unknown => "unknown" }
    }
    pub fn parse_target(s: &str, host: Distro) -> Option<Distro> {
        match s { "host" | "auto" => Some(host), "arch" => Some(Distro::Arch), "debian" => Some(Distro::Debian), _ => None }
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

/// Locate the bundled tune/patch set for standalone bundles, per-user installs, system installs,
/// and source checkouts. AKT_DATA_DIR is an explicit override; standalone data beside the invoked
/// binary takes precedence so a local bundle never silently uses unrelated system-wide config.
pub fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("AKT_DATA_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            if parent.join("config/arxos.fragment").is_file() {
                return parent.to_path_buf();
            }
        }
    }
    let user_data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join(".local/share")));
    if let Some(root) = user_data {
        let user = root.join("kernel-builder");
        if user.join("config/arxos.fragment").is_file() {
            return user;
        }
    }
    for p in ["/usr/share/kernel-builder", "/usr/share/arxos-kernel-tuner", env!("CARGO_MANIFEST_DIR")] {
        let pb = PathBuf::from(p);
        if pb.join("config/arxos.fragment").is_file() { return pb; }
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
    pub cpu_target: String,    // auto, portable, v1..v4
    pub hz: String,            // scheduler tick rate; verified against chosen source config
    pub hugepages: String,     // madvise or always
    pub performance_governor: bool, // opt-in; off by default for laptops
    pub max_jobs: Option<usize>, // optional cap, further clamped by resource planner
    pub thinlto_jobs: Option<usize>, // optional independent ThinLTO backend cap
    pub source_ref: Option<String>, // pinned upstream recipe ref for Arch builds
    pub base: String,          // upstream base we fork/patch
    pub tunes: Vec<String>,    // performance tunes applied
    pub overlays: Vec<String>, // ARXOS config fragments overlaid
    pub patch_sets: Vec<String>,
    pub extra_patches: Vec<String>, // user-supplied patches (files or dirs) from --patch, applied last
    pub patch_strict: bool,    // true (default): a patch that fails to apply ABORTS; false: skip it
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
                cpu_target: "auto".into(), hz: "1000".into(), hugepages: "madvise".into(), performance_governor: false, max_jobs: None, thinlto_jobs: None, source_ref: None,
                extra_patches: vec![], patch_strict: true,
                base: format!("{UPSTREAM} (rolling latest)"),
                tunes: vec![
                    format!("source knobs: scheduler={}, LTO={}, CPU target=auto, HZ=1000, THP=madvise, performance governor=off", fl.sched, fl.lto),
                ],
                overlays: vec![arxos_frag],
                patch_sets: vec![dd.join("patches/arxos").display().to_string()],
                output: format!("pacman package  linux-{kname} rev {pkgrel}  (+ headers){}", out.map(|o| format!("  ->  {o}")).unwrap_or_default()),
            },
            Distro::Debian => Plan {
                distro, flavor: fl, kname: kname.to_string(),
                out: out.map(|s| s.to_string()), pkgrel: pkgrel.to_string(),
                cpu_target: "auto".into(), hz: "1000".into(), hugepages: "madvise".into(), performance_governor: false, max_jobs: None, thinlto_jobs: None, source_ref: None,
                extra_patches: vec![], patch_strict: true,
                base: "Debian kernel source (apt source / linux-source)".into(),
                tunes: vec![
                    "Debian source: requested Kconfig options will be checked; BORE/BBRv3 are not assumed without their patches".into(),
                    format!("scheduler profile={}, LTO profile={}", fl.sched, fl.lto),
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
                cpu_target: "auto".into(), hz: "1000".into(), hugepages: "madvise".into(), performance_governor: false, max_jobs: None, thinlto_jobs: None, source_ref: None,
                extra_patches: vec![], patch_strict: true,
                base: "unknown host".into(), tunes: vec![], overlays: vec![], patch_sets: vec![],
                output: "unsupported — akt supports Arch-family and Debian-family hosts".into(),
            },
        }
    }
    /// Resolve hardware-aware tuning options shared by CLI, GUI and builder.
    pub fn with_tuning(mut self, cpu_target: &str, hz: &str, hugepages: &str, performance_governor: bool) -> Plan {
        self.cpu_target = cpu_target.to_string();
        self.hz = hz.to_string();
        self.hugepages = hugepages.to_string();
        self.performance_governor = performance_governor;
        if !self.tunes.is_empty() {
            let target = if cpu_target == "auto" {
                crate::sysinfo::SystemInfo::detect(std::path::Path::new(".")).recommended_cpu_target()
            } else if cpu_target == "portable" { "v1".to_string() } else { cpu_target.to_string() };
            let sched = self.flavor.sched;
            let lto = self.flavor.lto;
            let hz = self.hz.clone();
            let hugepages = self.hugepages.clone();
            let pgov = if self.performance_governor { "on (opt-in)" } else { "off" };
            let line = match self.distro {
                Distro::Arch => format!("source knobs: scheduler={sched}, LTO={lto}, CPU baseline={target}, HZ={hz}, THP={hugepages}, performance governor={pgov}"),
                Distro::Debian => format!("Debian Kconfig: CPU target={target}, HZ={hz}, THP={hugepages}, performance governor={pgov}; unavailable options must fail verification"),
                Distro::Unknown => String::new(),
            };
            self.tunes[0] = line;
        }
        self
    }
    /// Attach an optional user job cap; the resource planner still clamps it to detected limits.
    pub fn with_max_jobs(mut self, cap: Option<usize>) -> Plan {
        self.max_jobs = cap;
        if let Some(first) = self.tunes.first_mut() {
            let cap_label = cap.map(|v| v.to_string()).unwrap_or_else(|| "automatic".into());
            first.push_str(&format!(", requested max build jobs={cap_label}"));
        }
        self
    }
    pub fn with_thinlto_jobs(mut self, cap: Option<usize>) -> Plan {
        self.thinlto_jobs = cap;
        if let Some(first) = self.tunes.first_mut() {
            let label = cap.map(|v| v.to_string()).unwrap_or_else(|| "automatic".into());
            first.push_str(&format!(", requested ThinLTO jobs={label}"));
        }
        self
    }
    pub fn with_source_ref(mut self, source_ref: Option<String>) -> Plan {
        self.source_ref = source_ref;
        if let Some(first) = self.tunes.first_mut() {
            let label = self.source_ref.as_deref().unwrap_or("rolling");
            first.push_str(&format!(", source ref={label}"));
        }
        self
    }
    /// Attach user-supplied patches (from --patch) and the apply mode (strict = fail-fast).
    pub fn with_patches(mut self, extra: Vec<String>, strict: bool) -> Plan {
        self.extra_patches = extra;
        self.patch_strict = strict;
        self
    }
    pub fn to_json(&self) -> String {
        let arr = |v: &[String]| {
            let items: Vec<String> = v.iter().map(|s| format!("\"{}\"", json_escape(s))).collect();
            format!("[{}]", items.join(","))
        };
        format!(
            "{{\"distro\":\"{}\",\"flavor\":\"{}\",\"kernel\":\"{}\",\"pkgrel\":\"{}\",\"out\":\"{}\",\"cpu_target\":\"{}\",\"hz\":\"{}\",\"hugepages\":\"{}\",\"performance_governor\":{},\"max_jobs\":{},\"thinlto_jobs\":{},\"source_ref\":\"{}\",\"base\":\"{}\",\"tunes\":{},\"overlays\":{},\"patch_sets\":{},\"extra_patches\":{},\"patch_strict\":{},\"output\":\"{}\"}}",
            self.distro.name(), self.flavor.key, json_escape(&self.kname), json_escape(&self.pkgrel), json_escape(self.out.as_deref().unwrap_or("")),
            json_escape(&self.cpu_target), json_escape(&self.hz), json_escape(&self.hugepages), self.performance_governor,
            self.max_jobs.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            self.thinlto_jobs.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            json_escape(self.source_ref.as_deref().unwrap_or("")), json_escape(&self.base),
            arr(&self.tunes), arr(&self.overlays), arr(&self.patch_sets), arr(&self.extra_patches), self.patch_strict, json_escape(&self.output))
    }
}

/// Escape a string for a JSON string literal. Control characters MUST be escaped: a raw newline
/// inside a JSON string is invalid JSON, and every consumer rejects it. This only escaped the
/// backslash and the quote, which was survivable while every value was a single line, and silently
/// produced unparseable responses the moment a multi-line one appeared: a build log, a gate report,
/// the output of an install. The browser's own r.json() fails the same way, so the live log would
/// simply never update and nothing would say why.
/// How the upstream base is named in anything a user sees. ArxOS does not advertise which
/// distribution's recipe it forks on any user-facing surface, so every label, log line and error
/// message goes through this rather than hard-coding a vendor name. The clone URL necessarily still
/// resolves to that project, which is accepted: this is about not putting a third party's name in
/// front of our users, not about concealing where the code comes from from anyone who looks.
pub const UPSTREAM: &str = "tuned upstream kernel source";

/// Neutralise an internal path for display. The working checkout keeps the upstream directory names
/// because the recipe depends on them; what gets PRINTED should not carry the vendor name.
pub fn display_path(p: &str) -> String {
    p.replace("cos/linux-cachyos", "source").replace("/cos/", "/source/")
}

pub fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\u{08}' => o.push_str("\\b"),
            '\u{0c}' => o.push_str("\\f"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

#[cfg(test)]
mod json_escape_tests {
    use super::json_escape;
    #[test]
    fn control_characters_are_escaped_so_multiline_values_stay_valid_json() {
        assert_eq!(json_escape("a\nb"), "a\\nb");
        assert_eq!(json_escape("a\tb\r"), "a\\tb\\r");
        assert_eq!(json_escape("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(json_escape("c:\\path"), "c:\\\\path");
        assert_eq!(json_escape("\u{1}"), "\\u0001");
    }
    #[test]
    fn a_build_log_round_trips_through_a_json_string() {
        // The exact shape that used to break: a multi-line log embedded in a JSON object.
        let log = "== akt build ==\n  base : tuned upstream source\n  tune : \"quoted\"\n";
        let doc = format!("{{\"data\":\"{}\"}}", json_escape(log));
        assert!(!doc.contains('\n'), "no raw newline may survive inside the JSON string");
        assert!(doc.starts_with("{\"data\":\""), "doc = {doc}");
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}
fn has_flag(args: &[String], name: &str) -> bool { args.iter().any(|a| a == name) }
/// Collect every value of a repeatable flag, e.g. `--patch a.patch --patch b/`.
fn flags_multi<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    let mut v = vec![];
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            if let Some(val) = args.get(i + 1) { v.push(val.as_str()); i += 2; continue; }
        }
        i += 1;
    }
    v
}

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
    if cmd == "build" && has_flag(&args, "--output") {
        eprintln!("akt: '--output' is not supported; use '--out DIR' to select the package output directory");
        return ExitCode::FAILURE;
    }
    let host_distro = Distro::detect();
    let target_distro = match flag(&args, "--target-distro") {
        Some(raw) => match Distro::parse_target(raw, host_distro) {
            Some(d) => d,
            None => { eprintln!("akt: --target-distro must be host, arch, or debian"); return ExitCode::FAILURE; }
        },
        None => host_distro,
    };
    let cpu_target = flag(&args, "--cpu-target").unwrap_or("auto");
    if !matches!(cpu_target, "auto" | "portable" | "v1" | "v2" | "v3" | "v4") {
        eprintln!("akt: --cpu-target must be auto, portable, v1, v2, v3, or v4"); return ExitCode::FAILURE;
    }
    let hz = flag(&args, "--hz").unwrap_or("1000");
    if !matches!(hz, "1000" | "750" | "600" | "500" | "300" | "250" | "100") {
        eprintln!("akt: --hz must be one of 1000, 750, 600, 500, 300, 250, or 100"); return ExitCode::FAILURE;
    }
    let hugepages = flag(&args, "--hugepages").unwrap_or("madvise");
    if !matches!(hugepages, "madvise" | "always") {
        eprintln!("akt: --hugepages must be madvise or always"); return ExitCode::FAILURE;
    }
    let performance_governor = has_flag(&args, "--performance-governor");
    let max_jobs = match flag(&args, "--max-jobs") {
        Some("auto") => { std::env::remove_var("AKT_MAX_JOBS"); None }
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if n > 0 => { std::env::set_var("AKT_MAX_JOBS", n.to_string()); Some(n) }
            _ => { eprintln!("akt: --max-jobs must be auto or a positive integer"); return ExitCode::FAILURE; }
        },
        None => std::env::var("AKT_MAX_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0),
    };
    let thinlto_jobs = match flag(&args, "--thinlto-jobs") {
        Some("auto") => { std::env::remove_var("AKT_MAX_LTO_JOBS"); None }
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if n > 0 => { std::env::set_var("AKT_MAX_LTO_JOBS", n.to_string()); Some(n) }
            _ => { eprintln!("akt: --thinlto-jobs must be auto or a positive integer"); return ExitCode::FAILURE; }
        },
        None => std::env::var("AKT_MAX_LTO_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0),
    };
    let source_ref = flag(&args, "--source-ref").map(str::to_string);
    if let Some(ref_name) = &source_ref {
        let valid = !ref_name.is_empty() && ref_name.len() <= 200
            && ref_name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
            && !ref_name.starts_with('-') && !ref_name.starts_with('/') && !ref_name.ends_with('/')
            && !ref_name.contains("..") && !ref_name.contains("//");
        if !valid {
            eprintln!("akt: --source-ref must be a tag, branch, or commit reference using letters, digits, '.', '_', '-', and '/'");
            return ExitCode::FAILURE;
        }
        if target_distro != Distro::Arch {
            eprintln!("akt: --source-ref applies to the Arch source recipe; Debian chooses its source package version");
            return ExitCode::FAILURE;
        }
    }
    let json = has_flag(&args, "--json");
    let fl = flavor(flag(&args, "--flavor").unwrap_or("default")).unwrap_or(&FLAVORS[0]);
    let kname = match kernel_name(flag(&args, "--name").or_else(|| flag(&args, "--rename")).unwrap_or("arxos")) {
        Ok(n) => n,
        Err(e) => { eprintln!("akt: {e}"); return ExitCode::FAILURE; }
    };
    let out = flag(&args, "--out").map(|s| s.to_string());
    let extra_patches: Vec<String> = flags_multi(&args, "--patch").iter().map(|s| s.to_string()).collect();
    let patch_strict = !has_flag(&args, "--patch-optional");
    let pkgrel = flag(&args, "--pkgrel").unwrap_or("1").to_string();
    if pkgrel.is_empty() || !pkgrel.chars().all(|c| c.is_ascii_digit()) {
        eprintln!("akt: --pkgrel must be a number (the package revision / version, default 1)"); return ExitCode::FAILURE;
    }

    match cmd {
        "resources" => {
            let work = std::env::var("AKT_WORK").unwrap_or_else(|_| ".".into());
            match resources::detect(std::path::Path::new(&work)) {
                Ok(r) => { if json { println!("{}", r.json()); } else { r.report(); } ExitCode::SUCCESS }
                Err(e) => { eprintln!("akt: resources: {e}"); ExitCode::FAILURE }
            }
        }
        "detect" => {
            let d = Distro::detect();
            if json { println!("{{\"distro\":\"{}\",\"supported\":{}}}", d.name(), d != Distro::Unknown); }
            else {
                println!("host distro : {}", d.name());
                println!("build path  : {}", match d {
                    Distro::Arch => "Arch: fork the tuned upstream source + overlay ARXOS config/patches -> pacman package",
                    Distro::Debian => "Debian: Debian source + ArxOS tunes + ARXOS + Kali patches -> .deb",
                    Distro::Unknown => "unsupported (need an Arch-family or Debian-family host)",
                });
            }
            ExitCode::SUCCESS
        }
        "plan" => {
            let p = Plan::resolve(target_distro, fl, &kname, out.as_deref(), &pkgrel)
                .with_tuning(cpu_target, hz, hugepages, performance_governor)
                .with_max_jobs(max_jobs)
                .with_thinlto_jobs(thinlto_jobs)
                .with_source_ref(source_ref.clone())
                .with_patches(extra_patches.clone(), patch_strict);
            if json { println!("{}", p.to_json()); }
            else {
                println!("== akt tune plan ({}, flavor {}) ==", p.distro.name(), p.flavor.key);
                println!("  base     : {}", p.base);
                for t in &p.tunes { println!("  tune     : {t}"); }
                for o in &p.overlays { println!("  overlay  : {o}"); }
                for ps in &p.patch_sets { println!("  patches  : {ps}"); }
                for ep in &p.extra_patches { println!("  +patch   : {ep}"); }
                println!("  patch mode: {}", if p.patch_strict { "strict (fail-fast)" } else { "optional (skip on miss)" });
                println!("  output   : {}", p.output);
            }
            ExitCode::SUCCESS
        }
        "patches" => {
            let p = Plan::resolve(target_distro, fl, &kname, out.as_deref(), &pkgrel)
                .with_tuning(cpu_target, hz, hugepages, performance_governor)
                .with_max_jobs(max_jobs)
                .with_thinlto_jobs(thinlto_jobs)
                .with_source_ref(source_ref.clone())
                .with_patches(extra_patches.clone(), patch_strict);
            let (files, dirs) = build::collect_patches(&build::bundled_patch_dirs(p.distro), &p.extra_patches);
            if json {
                let arr = |v: &[String]| { let i: Vec<String> = v.iter().map(|s| format!("\"{}\"", json_escape(s))).collect(); format!("[{}]", i.join(",")) };
                println!("{{\"patches\":{},\"strict\":{}}}", arr(&files), p.patch_strict);
            } else {
                println!("== patches for {} / flavor {} ==", p.distro.name(), p.flavor.key);
                if files.is_empty() {
                    println!("  (none — bundled dirs are empty; add with --patch <file|dir> or drop *.patch into patches/arxos)");
                }
                for f in &files { println!("  {f}"); }
                for d in &dirs {
                    if std::path::Path::new(&format!("{d}/SHA256SUMS")).exists() { println!("  integrity: {d}/SHA256SUMS present (verified before apply)"); }
                }
                println!("  order: bundled first, then --patch (files/dirs) in the order given");
                println!("  mode : {}", if p.patch_strict { "strict — a patch that fails to apply ABORTS the build" } else { "optional — patches that do not apply are skipped" });
            }
            ExitCode::SUCCESS
        }
        "explain" => {
            explain::print(target_distro, fl, &kname, json);
            ExitCode::SUCCESS
        }
        "system" | "recommend" => {
            let work = build::work_dir(host_distro, "default").unwrap_or_else(|_| std::path::PathBuf::from("."));
            let info = sysinfo::SystemInfo::detect(&work);
            if json { println!("{}", info.to_json()); } else { info.report(); }
            ExitCode::SUCCESS
        }
        "build" => {
            let dry = has_flag(&args, "--dry-run");
            let prepare_only = has_flag(&args, "--prepare-only");
            if prepare_only && (dry || target_distro != Distro::Arch || host_distro != Distro::Arch) {
                eprintln!("akt: --prepare-only requires an Arch-family host and Arch target, and cannot be combined with --dry-run");
                return ExitCode::FAILURE;
            }
            let plan = Plan::resolve(target_distro, fl, &kname, out.as_deref(), &pkgrel)
                .with_tuning(cpu_target, hz, hugepages, performance_governor)
                .with_max_jobs(max_jobs)
                .with_thinlto_jobs(thinlto_jobs)
                .with_source_ref(source_ref.clone())
                .with_patches(extra_patches.clone(), patch_strict);
            let result = if target_distro != host_distro && !dry && !prepare_only {
                container_build::run(&plan, &args, host_distro)
            } else {
                build::run(plan, dry, prepare_only)
            };
            match result {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => { eprintln!("akt: build failed: {e}"); ExitCode::FAILURE }
            }
        }
        "install" => {
            // Installs into the RUNNING system, so the running kernel is made restorable first.
            let keep = !args.iter().any(|a| a == "--no-keep-running");
            let plan = Plan::resolve(target_distro, fl, &kname, out.as_deref(), &pkgrel);
            match install::install_to_system(&plan, keep) {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => { eprintln!("akt: install failed: {e}"); ExitCode::FAILURE }
            }
        }
        "rollback" => {
            if target_distro != host_distro {
                eprintln!("akt: rollback operates on host-installed packages; do not set --target-distro for rollback");
                return ExitCode::FAILURE;
            }
            let plan = Plan::resolve(target_distro, fl, &kname, out.as_deref(), &pkgrel);
            match build::rollback(&plan) {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => { eprintln!("akt: rollback failed: {e}"); ExitCode::FAILURE }
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

  detect                 show the host distro + which native build path applies
  system [--json]        detect hardware and show build recommendations
  resources [--json]      inspect build CPU, RAM and disk limits (AKT_WORK path)
  plan   [--target-distro host|arch|debian] [--flavor F] print the selected plan
  patches [--patch P]    list every patch that will be applied, in order, for this host
  build  [--target-distro host|arch|debian] [--cpu-target auto|portable|v1|v2|v3|v4]
         [--hz HZ] [--hugepages madvise|always] [--max-jobs auto|N]
         [--thinlto-jobs auto|N] [--source-ref REF] [--performance-governor]
         [--flavor F] [--name NAME] [--pkgrel N] [--out DIR] [--patch P] [--dry-run | --prepare-only]
                         build a kernel package; cross-distro builds use Podman/Docker
                         --name / --rename  builds linux-<NAME> (default: arxos)
                         --target-distro    host (default), arch, or debian; cross-target needs Podman/Docker
                         --cpu-target       auto detects local CPU level; portable is for shared packages
                         --hz HZ            tick rate: 1000, 750, 600, 500, 300, 250, or 100
                         --hugepages        madvise (default) or always
                         --max-jobs         auto lets the planner decide; N caps jobs and is further resource-limited
                         --thinlto-jobs     auto or independent ThinLTO link-backend cap
                         --source-ref       pin the Arch source recipe to a tag, branch, or commit; default tracks rolling source
                         --performance-governor opt in to the performance governor default (not recommended for most laptops)
                         --pkgrel N         package revision / custom version (default: 1)
                         --out DIR          write the finished packages to DIR
                         --patch P          extra patch to apply (a .patch/.diff file OR a
                                            directory of them); repeatable, applied after the
                                            bundled patches in the order given
                         --patch-optional   skip patches that do not apply instead of aborting
                                            (default is strict: a failed patch aborts the build)
  explain [--flavor F] [--name NAME]
                         explain what each option means for your custom kernel
  install [--name NAME] [--out DIR] [--no-keep-running]
                         install the built kernel into THIS system. The kernel you are running is
                         made restorable FIRST: if no package file for it exists anywhere, one is
                         rebuilt from what the package manager records as installed, and it is put
                         back automatically if the install removes it. Arch and Debian today;
                         Fedora and others are planned, each needing its own package-manager arm.
  rollback [--name NAME] reinstall the previous linux-<NAME> package from prev/ (kernel rollback).
                         A same-name upgrade removes the old version's modules, so the retained
                         prev/ package is the reliable rollback; boot-test in a throwaway first.
  gate   <config-file>   verify the ARXOS hardening delta is present in a built .config
  gui    [--port N]      launch the local web GUI (default http://127.0.0.1:8787)
  version

PATCHES: bundled patches live in patches/arxos (all hosts) and patches/kali (Debian). A SHA256SUMS
file in a patch directory is verified before any patch is applied. Patches are auto-detected at -p1
or -p0, already-applied patches are skipped, and in strict mode (default) a patch that does not apply
aborts the build so a kernel is never silently shipped without an expected fix.

FLAVORS: default (BORE+ThinLTO) | rt (PREEMPT_RT) | hardened (KSPP+kCFI)
Add --json to detect/plan/gate for machine-readable output.")
}
