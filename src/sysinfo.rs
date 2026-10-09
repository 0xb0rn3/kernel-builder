//! Read-only Linux hardware inventory and conservative build recommendations.
//! Missing procfs/sysfs fields remain unknown. This module never changes host settings.
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}
fn parse_meminfo(key: &str) -> Option<u64> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    let kb = text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name != key { return None; }
        value.split_whitespace().next()?.parse::<u64>().ok()
    })?;
    Some(kb / 1024)
}
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\''))) {
        s[1..s.len() - 1].to_string()
    } else { s.to_string() }
}
fn os_release() -> (String, String, String) {
    let text = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let mut id = String::new();
    let mut pretty = String::new();
    let mut like = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("ID=") { id = unquote(v); }
        else if let Some(v) = line.strip_prefix("ID_LIKE=") { like = unquote(v); }
        else if let Some(v) = line.strip_prefix("PRETTY_NAME=") { pretty = unquote(v); }
    }
    if pretty.is_empty() { pretty = id.clone(); }
    (id, like, pretty)
}
fn cpu_blocks() -> Vec<Vec<(String, String)>> {
    fs::read_to_string("/proc/cpuinfo").unwrap_or_default().split("\n\n").filter_map(|block| {
        let fields: Vec<(String, String)> = block.lines().filter_map(|line| {
            let (k, v) = line.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        }).collect();
        if fields.iter().any(|(k, _)| k == "processor") { Some(fields) } else { None }
    }).collect()
}
fn field<'a>(cpu: &'a [(String, String)], name: &str) -> Option<&'a str> {
    cpu.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}
fn cpu_flags() -> BTreeSet<String> {
    let mut intersection: Option<HashSet<String>> = None;
    for cpu in cpu_blocks().iter() {
        let raw = field(cpu, "flags").or_else(|| field(cpu, "Features")).unwrap_or("");
        let here: HashSet<String> = raw.split_whitespace().map(str::to_string).collect();
        intersection = Some(match intersection {
            None => here,
            Some(old) => old.intersection(&here).cloned().collect(),
        });
    }
    intersection.unwrap_or_default().into_iter().collect()
}
fn x86_64_level(flags: &BTreeSet<String>) -> u8 {
    if !cfg!(target_arch = "x86_64") { return 0; }
    const V2: &[&str] = &["cx16", "lahf_lm", "popcnt", "pni", "sse4_1", "sse4_2", "ssse3"];
    const V3: &[&str] = &["avx", "avx2", "bmi1", "bmi2", "f16c", "fma", "lzcnt", "movbe", "xsave"];
    const V4: &[&str] = &["avx512f", "avx512bw", "avx512cd", "avx512dq", "avx512vl"];
    if !V2.iter().all(|f| flags.contains(*f)) { return 1; }
    if !V3.iter().all(|f| flags.contains(*f)) { return 2; }
    if !V4.iter().all(|f| flags.contains(*f)) { return 3; }
    4
}
fn level_label(level: u8) -> &'static str {
    match level {
        4 => "x86-64-v4", 3 => "x86-64-v3", 2 => "x86-64-v2",
        1 => "x86-64-v1 (baseline)", _ => "non-x86_64 / unknown",
    }
}
fn frequency_khz(name: &str) -> Vec<u64> {
    let mut values = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/devices/system/cpu") {
        for entry in entries.flatten() {
            let n = entry.file_name().to_string_lossy().to_string();
            if n.starts_with("cpu") && n[3..].chars().all(|c| c.is_ascii_digit()) {
                if let Some(v) = read_trim(&entry.path().join("cpufreq").join(name)).and_then(|v| v.parse().ok()) {
                    values.push(v);
                }
            }
        }
    }
    values.sort_unstable();
    values
}
fn first_cpu_text(name: &str) -> Option<String> {
    let entries = fs::read_dir("/sys/devices/system/cpu").ok()?;
    for entry in entries.flatten() {
        let n = entry.file_name().to_string_lossy().to_string();
        if n.starts_with("cpu") && n[3..].chars().all(|c| c.is_ascii_digit()) {
            if let Some(s) = read_trim(&entry.path().join("cpufreq").join(name)) { return Some(s); }
        }
    }
    None
}
fn physical_cores(cpus: &[Vec<(String, String)>]) -> Option<usize> {
    let mut ids = BTreeSet::new();
    for cpu in cpus {
        if let (Some(package), Some(core)) = (field(cpu, "physical id"), field(cpu, "core id")) {
            ids.insert((package.to_string(), core.to_string()));
        }
    }
    if ids.is_empty() {
        cpus.first().and_then(|c| field(c, "cpu cores")).and_then(|v| v.parse().ok())
    } else { Some(ids.len()) }
}
fn temperatures() -> Vec<(String, f64, Option<f64>)> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/thermal") else { return out; };
    for entry in entries.flatten() {
        let n = entry.file_name().to_string_lossy().to_string();
        if !n.starts_with("thermal_zone") { continue; }
        let p = entry.path();
        let Some(kind) = read_trim(&p.join("type")) else { continue; };
        let Some(raw) = read_trim(&p.join("temp")).and_then(|v| v.parse::<f64>().ok()) else { continue; };
        let c = if raw.abs() > 1000.0 { raw / 1000.0 } else { raw };
        if !(-20.0..=150.0).contains(&c) { continue; }
        let mut critical = None;
        if let Ok(trips) = fs::read_dir(&p) {
            for trip in trips.flatten() {
                let name = trip.file_name().to_string_lossy().to_string();
                if !name.starts_with("trip_point_") || !name.ends_with("_type") { continue; }
                if read_trim(&trip.path()).as_deref() == Some("critical") {
                    let base = name.trim_end_matches("_type");
                    critical = read_trim(&p.join(format!("{base}_temp"))).and_then(|v| v.parse::<f64>().ok())
                        .map(|v| if v.abs() > 1000.0 { v / 1000.0 } else { v });
                    break;
                }
            }
        }
        out.push((kind, c, critical));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
fn gpus() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/drm") else { return out; };
    for entry in entries.flatten() {
        let n = entry.file_name().to_string_lossy().to_string();
        if !n.starts_with("card") || !n[4..].chars().all(|c| c.is_ascii_digit()) { continue; }
        let p = entry.path().join("device");
        let vendor = read_trim(&p.join("vendor"));
        let id = read_trim(&p.join("device"));
        if vendor.is_none() && id.is_none() { continue; }
        let driver = fs::read_link(p.join("driver")).ok()
            .and_then(|v| v.file_name().map(|s| s.to_string_lossy().to_string()));
        let desc = format!("{}{}{}", vendor.map(|v| format!("vendor {v}")).unwrap_or_default(),
            id.map(|v| format!(" device {v}")).unwrap_or_default(),
            driver.map(|v| format!(" driver {v}")).unwrap_or_default());
        if !out.contains(&desc) { out.push(desc); }
    }
    out.sort();
    out
}
fn battery_info() -> (Option<u8>, Option<String>, Option<bool>) {
    let mut capacity = None;
    let mut status = None;
    let mut ac_online = None;
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else { return (capacity, status, ac_online); };
    for entry in entries.flatten() {
        let p = entry.path();
        match read_trim(&p.join("type")).as_deref() {
            Some("Battery") => {
                capacity = read_trim(&p.join("capacity")).and_then(|s| s.parse::<u8>().ok()).filter(|v| *v <= 100);
                status = read_trim(&p.join("status"));
            }
            Some("Mains") | Some("USB") | Some("USB_C") => {
                if let Some(v) = read_trim(&p.join("online")).and_then(|s| s.parse::<u8>().ok()) {
                    if v == 1 { ac_online = Some(true); } else if ac_online.is_none() { ac_online = Some(false); }
                }
            }
            _ => {}
        }
    }
    (capacity, status, ac_online)
}
fn disk_free_gib(path: &Path) -> Option<u64> {
    let mut existing = path.to_path_buf();
    while !existing.exists() { if !existing.pop() { return None; } }
    let output = Command::new("df").args(["-Pk", "--"]).arg(&existing).env("LC_ALL", "C").output().ok()?;
    if !output.status.success() { return None; }
    let kb = String::from_utf8_lossy(&output.stdout).lines().last()?.split_whitespace().nth(3)?.parse::<u64>().ok()?;
    Some(kb / (1024 * 1024))
}
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""), '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"), '\r' => out.push_str("\\r"), '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[derive(Clone, Debug)]
pub struct SystemInfo {
    pub os_id: String, pub os_like: String, pub os_pretty: String, pub arch: String, pub machine: String,
    pub cpu_vendor: String, pub cpu_model: String, pub logical_cpus: usize, pub physical_cores: Option<usize>,
    pub x86_level: u8, pub ram_total_mib: Option<u64>, pub ram_available_mib: Option<u64>, pub swap_total_mib: Option<u64>,
    pub cpu_max_mhz: Option<u64>, pub cpu_governor: Option<String>, pub cpu_driver: Option<String>,
    pub temperatures: Vec<(String, f64, Option<f64>)>, pub gpus: Vec<String>,
    pub battery_percent: Option<u8>, pub battery_status: Option<String>, pub ac_online: Option<bool>, pub work_free_gib: Option<u64>,
}
impl SystemInfo {
    pub fn detect(work: &Path) -> Self {
        let (os_id, os_like, os_pretty) = os_release();
        let cpus = cpu_blocks();
        let cpu_model = cpus.first().and_then(|c| field(c, "model name").or_else(|| field(c, "Processor"))).unwrap_or("unknown CPU").to_string();
        let cpu_vendor = cpus.first().and_then(|c| field(c, "vendor_id").or_else(|| field(c, "CPU implementer"))).unwrap_or("unknown vendor").to_string();
        let flags = cpu_flags();
        let level = x86_64_level(&flags);
        let logical_cpus = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(cpus.len().max(1));
        let machine = [
            read_trim(Path::new("/sys/class/dmi/id/sys_vendor")),
            read_trim(Path::new("/sys/class/dmi/id/product_name")),
            read_trim(Path::new("/sys/class/dmi/id/product_version")),
        ].into_iter().flatten().filter(|s| !s.is_empty() && s != "None" && s != "To Be Filled By O.E.M.")
          .collect::<Vec<_>>().join(" ");
        let (battery_percent, battery_status, ac_online) = battery_info();
        Self {
            os_id, os_like, os_pretty, arch: std::env::consts::ARCH.to_string(),
            machine: if machine.is_empty() { "hardware model unavailable".to_string() } else { machine },
            cpu_vendor, cpu_model, logical_cpus, physical_cores: physical_cores(&cpus), x86_level: level,
            ram_total_mib: parse_meminfo("MemTotal"), ram_available_mib: parse_meminfo("MemAvailable"),
            swap_total_mib: parse_meminfo("SwapTotal"),
            cpu_max_mhz: frequency_khz("cpuinfo_max_freq").last().copied().map(|v| v / 1000),
            cpu_governor: first_cpu_text("scaling_governor"), cpu_driver: first_cpu_text("scaling_driver"),
            temperatures: temperatures(), gpus: gpus(), battery_percent, battery_status, ac_online,
            work_free_gib: disk_free_gib(work),
        }
    }
    pub fn recommended_cpu_target(&self) -> String {
        if self.x86_level == 0 { "portable".to_string() } else { format!("v{}", self.x86_level) }
    }
    pub fn recommendations(&self) -> Vec<String> {
        let mut v = Vec::new();
        let target = self.recommended_cpu_target();
        if target == "portable" {
            v.push("Use the portable x86-64 baseline; this host did not expose x86-64-v2/v3/v4 capability data.".to_string());
        } else {
            v.push(format!("For this machine only, CPU flags indicate {target} as the highest supported x86-64 baseline. Such a kernel may not boot on older CPUs; use --cpu-target portable for broadly shareable packages."));
        }
        v.push("Keep ThinLTO as the starting point; benchmark against no LTO with the same workload before claiming a runtime gain.".to_string());
        v.push("Use THP=madvise as the laptop-friendly starting point; choose always only after memory-workload benchmarks.".to_string());
        v.push("Leave the kernel's performance-governor default disabled on laptops unless you deliberately prioritize responsiveness over power use.".to_string());
        if self.logical_cpus < 4 || self.ram_total_mib.unwrap_or(0) < 8192 {
            v.push("Conservative build ceiling: start with AKT_MAX_JOBS=2 to preserve memory and desktop responsiveness.".to_string());
        } else if self.logical_cpus >= 8 && self.ram_total_mib.unwrap_or(0) >= 16384 {
            v.push("Use automatic resource planning first; if sustained thermal throttling occurs, compare AKT_MAX_JOBS=4 against the automatic setting.".to_string());
        } else {
            v.push("Use automatic resource planning, lowering AKT_MAX_JOBS if the machine becomes unresponsive or thermally throttles.".to_string());
        }
        if self.work_free_gib.unwrap_or(0) < 60 {
            v.push("The visible work filesystem has less than 60 GiB free or could not be measured; choose a workspace with at least 60 GiB free.".to_string());
        }
        if let Some((kind, temp, critical)) = self.temperatures.iter()
            .filter(|(kind, _, _)| { let k = kind.to_ascii_lowercase(); k.contains("cpu") || k.contains("pkg") || k.contains("x86") || k.contains("core") || k.contains("acpitz") })
            .max_by(|a, b| a.1.total_cmp(&b.1)) {
            if critical.as_ref().map(|c| *temp >= *c - 10.0).unwrap_or(*temp >= 90.0) {
                v.push(format!("Thermal caution: sensor {kind} reports {temp:.1}°C. Re-check the sensor and cooling before a sustained build; this is a warning, not a hardware safety certification."));
            }
        }
        if self.ac_online == Some(false) && self.battery_percent.unwrap_or(100) < 35 {
            v.push("Battery caution: low charge and no AC power detected. Avoid starting a long kernel build until connected to power.".to_string());
        }
        if self.cpu_governor.is_none() {
            v.push("CPU frequency policy was not exposed through sysfs; governor behavior is unknown, so no recommendation is inferred.".to_string());
        }
        v
    }
    pub fn to_json(&self) -> String {
        let temps = self.temperatures.iter().map(|(kind, c, critical)| {
            format!("{{\"type\":{},\"celsius\":{:.1},\"critical_celsius\":{}}}", json_str(kind), c,
                critical.map(|v| format!("{v:.1}")).unwrap_or_else(|| "null".to_string()))
        }).collect::<Vec<_>>().join(",");
        let gpus = self.gpus.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(",");
        let recs = self.recommendations().iter().map(|s| json_str(s)).collect::<Vec<_>>().join(",");
        format!(
            "{{\"os_id\":{},\"os_like\":{},\"os_pretty\":{},\"arch\":{},\"machine\":{},\"cpu_vendor\":{},\"cpu_model\":{},\"logical_cpus\":{},\"physical_cores\":{},\"x86_64_level\":{},\"x86_64_level_label\":{},\"recommended_cpu_target\":{},\"ram_total_mib\":{},\"ram_available_mib\":{},\"swap_total_mib\":{},\"cpu_max_mhz\":{},\"cpu_governor\":{},\"cpu_driver\":{},\"temperatures\":[{}],\"gpus\":[{}],\"battery_percent\":{},\"battery_status\":{},\"ac_online\":{},\"work_free_gib\":{},\"recommendations\":[{}]}}",
            json_str(&self.os_id), json_str(&self.os_like), json_str(&self.os_pretty), json_str(&self.arch), json_str(&self.machine),
            json_str(&self.cpu_vendor), json_str(&self.cpu_model), self.logical_cpus,
            self.physical_cores.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()), self.x86_level,
            json_str(level_label(self.x86_level)), json_str(&self.recommended_cpu_target()),
            self.ram_total_mib.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            self.ram_available_mib.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            self.swap_total_mib.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            self.cpu_max_mhz.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            self.cpu_governor.as_deref().map(json_str).unwrap_or_else(|| "null".to_string()),
            self.cpu_driver.as_deref().map(json_str).unwrap_or_else(|| "null".to_string()), temps, gpus,
            self.battery_percent.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            self.battery_status.as_deref().map(json_str).unwrap_or_else(|| "null".to_string()),
            self.ac_online.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            self.work_free_gib.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()), recs)
    }
    pub fn report(&self) {
        println!("== hardware detected ==");
        println!("  OS         : {} ({}, {})", self.os_pretty, self.os_id, self.arch);
        println!("  machine    : {}", self.machine);
        println!("  CPU        : {} ({})", self.cpu_model, self.cpu_vendor);
        println!("  cores      : {} physical / {} logical", self.physical_cores.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()), self.logical_cpus);
        println!("  ISA target : {} (recommended for this machine only)", level_label(self.x86_level));
        println!("  RAM        : {} MiB total / {} MiB available", self.ram_total_mib.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()), self.ram_available_mib.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()));
        println!("  swap       : {} MiB", self.swap_total_mib.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()));
        println!("  CPU policy : governor={} driver={} max-frequency={} MHz", self.cpu_governor.as_deref().unwrap_or("unknown"), self.cpu_driver.as_deref().unwrap_or("unknown"), self.cpu_max_mhz.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()));
        println!("  work disk  : {} GiB free", self.work_free_gib.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()));
        for (kind, c, critical) in &self.temperatures {
            println!("  thermal    : {kind} {c:.1}°C{}", critical.map(|v| format!(" (critical {v:.1}°C)")).unwrap_or_default());
        }
        for g in &self.gpus { println!("  GPU        : {g}"); }
        if let Some(v) = self.battery_percent { println!("  battery    : {v}% ({})", self.battery_status.as_deref().unwrap_or("unknown")); }
        if let Some(v) = self.ac_online { println!("  AC power   : {}", if v { "connected" } else { "not detected" }); }
        println!("\n== conservative recommendations (not a hardware safety guarantee) ==");
        for r in self.recommendations() { println!("  - {r}"); }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_level_requires_complete_feature_groups() {
        let v2: BTreeSet<String> = ["cx16", "lahf_lm", "popcnt", "pni", "sse4_1", "sse4_2", "ssse3"].iter().map(|v| v.to_string()).collect();
        assert_eq!(x86_64_level(&v2), 2);
        let mut v3 = v2.clone();
        for v in ["avx", "avx2", "bmi1", "bmi2", "f16c", "fma", "lzcnt", "movbe", "xsave"] { v3.insert(v.to_string()); }
        assert_eq!(x86_64_level(&v3), 3);
        let mut v4 = v3.clone();
        for v in ["avx512f", "avx512bw", "avx512cd", "avx512dq", "avx512vl"] { v4.insert(v.to_string()); }
        assert_eq!(x86_64_level(&v4), 4);
    }
    #[test]
    fn json_escapes_control_characters() {
        assert_eq!(json_str("a\n\"b"), "\"a\\n\\\"b\"");
    }
}
