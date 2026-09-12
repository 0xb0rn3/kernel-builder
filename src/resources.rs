// Resource planning shared with linux-arxos. std-only; also a standalone helper.
use std::{env, fs, io, path::Path, process::Command};

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Debug)]
pub struct Resources {
    pub cpus: usize,
    pub available: u64,
    pub reserve: u64,
    pub disk: u64,
    pub jobs: usize,
    pub link_jobs: usize,
}

pub fn calculate(cpus: usize, available: u64, disk: u64, cap: Option<usize>) -> io::Result<Resources> {
    if cpus == 0 || cap == Some(0) { return Err(invalid("CPU/job count must be positive")); }
    let reserve = (available / 4).clamp(512 * MIB, 4 * GIB);
    let memory_jobs = available.saturating_sub(reserve) / (2 * GIB);
    if memory_jobs == 0 { return Err(invalid("not enough available RAM: need 2 GiB per job plus host reserve")); }
    if disk < 60 * GIB { return Err(invalid("kernel work filesystem needs at least 60 GiB free")); }
    // Leave one logical CPU of headroom when possible. Never raise the user's cap.
    let jobs = cpus.saturating_sub(1).max(1).min(memory_jobs as usize).min(cap.unwrap_or(usize::MAX));
    Ok(Resources { cpus, available, reserve, disk, jobs, link_jobs: jobs.min(4) })
}

fn mem_available(text: &str) -> io::Result<u64> {
    let kb = text.lines().find_map(|l| l.strip_prefix("MemAvailable:"))
        .and_then(|l| l.split_whitespace().next()).and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| invalid("/proc/meminfo has no valid MemAvailable"))?;
    kb.checked_mul(1024).ok_or_else(|| invalid("MemAvailable overflow"))
}

// Read every visible v2 ancestor, since parent limits constrain the child too.
fn cgroup_memory(root: &Path, membership: &str, mut available: u64) -> io::Result<u64> {
    let relative = membership.lines().find_map(|l| l.strip_prefix("0::"))
        .ok_or_else(|| invalid("cgroup v2 required for automatic memory-limit detection"))?;
    if !relative.starts_with('/') || relative.split('/').any(|v| v == "..") {
        return Err(invalid("unresolvable cgroup namespace path"));
    }
    let mut dir = root.join(relative.trim_start_matches('/'));
    if !dir.is_dir() { return Err(invalid("cgroup path is not visible; cannot safely size jobs")); }
    loop {
        for name in ["memory.max", "memory.high"] {
            let limit = match fs::read_to_string(dir.join(name)) {
                Ok(v) if v.trim() == "max" => continue,
                Ok(v) => v.trim().parse::<u64>().map_err(|_| invalid("invalid cgroup memory limit"))?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let used = fs::read_to_string(dir.join("memory.current"))?.trim().parse::<u64>()
                .map_err(|_| invalid("invalid cgroup memory.current"))?;
            available = available.min(limit.saturating_sub(used));
        }
        if dir == root { break; }
        if !dir.pop() || !dir.starts_with(root) { return Err(invalid("cgroup escaped mount")); }
    }
    Ok(available)
}

pub fn detect(work: &Path) -> io::Result<Resources> {
    let cpus = std::thread::available_parallelism()?.get();
    let available = cgroup_memory(Path::new("/sys/fs/cgroup"), &fs::read_to_string("/proc/self/cgroup")?,
        mem_available(&fs::read_to_string("/proc/meminfo")?)?)?;
    let mut existing = work.to_path_buf();
    while !existing.exists() {
        if !existing.pop() { existing = env::current_dir()?; break; }
    }
    let output = Command::new("df").args(["-Pk", "--"]).arg(&existing).env("LC_ALL", "C").output()?;
    if !output.status.success() { return Err(invalid("cannot read work filesystem free space")); }
    let disk = String::from_utf8_lossy(&output.stdout).lines().last()
        .and_then(|l| l.split_whitespace().nth(3)).and_then(|v| v.parse::<u64>().ok())
        .and_then(|v| v.checked_mul(1024)).ok_or_else(|| invalid("invalid df output"))?;
    let cap = match env::var("AKT_MAX_JOBS") {
        Ok(s) => Some(s.parse::<usize>().map_err(|_| invalid("AKT_MAX_JOBS must be a positive integer"))?),
        Err(env::VarError::NotPresent) => None,
        Err(_) => return Err(invalid("invalid AKT_MAX_JOBS")),
    };
    calculate(cpus, available, disk, cap)
}

impl Resources {
    pub fn json(&self) -> String {
        format!("{{\"cpus\":{},\"available_bytes\":{},\"reserve_bytes\":{},\"disk_free_bytes\":{},\"jobs\":{},\"thinlto_jobs\":{}}}",
            self.cpus, self.available, self.reserve, self.disk, self.jobs, self.link_jobs)
    }
    pub fn report(&self) {
        eprintln!("resources: CPUs={} available={}MiB reserve={}MiB disk={}GiB make=-j{} ThinLTO={}",
            self.cpus, self.available / MIB, self.reserve / MIB, self.disk / GIB, self.jobs, self.link_jobs);
    }
}

pub fn recipe_text(text: &str) -> io::Result<String> {
    let old = "-j\"$(nproc)\"";
    let new = "-j\"${ARXOS_BUILD_JOBS:?resource plan missing}\"";
    if !text.contains(old) && !text.contains(new) {
        return Err(invalid("unknown recipe job syntax; refusing an uncapped build"));
    }
    let result = text.replace(old, new);
    // A rolling recipe with additional CPU-count jobs needs review, not silent bypass.
    if result.contains("nproc") { return Err(invalid("unreviewed nproc use in recipe")); }
    Ok(result)
}

pub fn limit_recipe(path: &Path) -> io::Result<()> {
    let text = recipe_text(&fs::read_to_string(path)?)?;
    fs::write(path, text)
}

pub fn kernel_text(text: &str, jobs: usize) -> io::Result<String> {
    if jobs == 0 { return Err(invalid("link jobs must be positive")); }
    let anchor = "ifdef CONFIG_LTO_CLANG_THIN\n";
    let marker = "# AKT bounded ThinLTO\n";
    if text.contains(marker) { return Err(invalid("kernel link limit already applied; use fresh source")); }
    if text.matches(anchor).count() != 1 { return Err(invalid("unknown kernel ThinLTO Makefile layout")); }
    // Append, do not replace architecture/ABI KBUILD_LDFLAGS. Only active for ThinLTO.
    Ok(text.replacen(anchor, &format!("{anchor}{marker}KBUILD_LDFLAGS += --thinlto-jobs={jobs}\n"), 1))
}

pub fn limit_kernel(dir: &Path, jobs: usize) -> io::Result<()> {
    if fs::read_to_string(dir.join(".config"))?.lines().any(|l| l == "CONFIG_LTO_CLANG_THIN=y") {
        let path = dir.join("Makefile");
        fs::write(&path, kernel_text(&fs::read_to_string(&path)?, jobs)?)?;
    }
    Ok(())
}

// Built standalone by linux-arxos; akt calls the same functions natively.
#[allow(dead_code)]
fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("plan") => {
            let r = detect(Path::new(args.get(2).map(String::as_str).unwrap_or(".")))?;
            r.report();
            if args.iter().any(|v| v == "--json") { println!("{}", r.json()); }
            else { println!("{} {}", r.jobs, r.link_jobs); }
        }
        Some("recipe") if args.len() == 3 => limit_recipe(Path::new(&args[2]))?,
        Some("kernel") if args.len() == 4 => limit_kernel(Path::new(&args[2]),
            args[3].parse().map_err(|_| invalid("invalid link jobs"))?)?,
        _ => return Err(invalid("usage: resources plan [WORK] [--json] | recipe PKGBUILD | kernel KDIR JOBS")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn roomy_laptop() {
        let r = calculate(8, 27 * GIB, 664 * GIB, None).unwrap();
        assert_eq!((r.jobs, r.link_jobs, r.reserve), (7, 4, 4 * GIB));
    }
    #[test] fn memory_constrains_large_cpu() {
        assert_eq!(calculate(128, 8 * GIB, 100 * GIB, None).unwrap().jobs, 3);
    }
    #[test] fn single_cpu_and_downward_cap() {
        assert_eq!(calculate(1, 8 * GIB, 100 * GIB, None).unwrap().jobs, 1);
        assert_eq!(calculate(8, 27 * GIB, 100 * GIB, Some(2)).unwrap().jobs, 2);
        assert_eq!(calculate(8, 27 * GIB, 100 * GIB, Some(999)).unwrap().jobs, 7);
    }
    #[test] fn insufficient_resources_fail() {
        assert!(calculate(8, 2 * GIB, 100 * GIB, None).is_err());
        assert!(calculate(8, 27 * GIB, 59 * GIB, None).is_err());
        assert!(calculate(8, 27 * GIB, 100 * GIB, Some(0)).is_err());
        assert!(calculate(0, 27 * GIB, 100 * GIB, None).is_err());
    }
    #[test] fn meminfo_requires_available_not_swap_or_total() {
        assert_eq!(mem_available("MemTotal: 999 kB\nMemAvailable: 123 kB\nSwapFree: 99999 kB\n").unwrap(), 123 * 1024);
        assert!(mem_available("MemTotal: 999 kB").is_err());
    }
    #[test] fn caps_both_recipe_jobs_idempotently() {
        let r = recipe_text("make -j\"$(nproc)\" all\nmake -j\"$(nproc)\" modules\n").unwrap();
        assert_eq!(r.matches("ARXOS_BUILD_JOBS").count(), 2);
        assert_eq!(recipe_text(&r).unwrap(), r);
        assert!(recipe_text("make -j$(nproc) all").is_err());
        assert!(recipe_text("make -j\"$(nproc)\" all\nx=$(nproc)").is_err());
    }
    #[test] fn link_limit_keeps_arch_flags_and_checks_layout() {
        let src = "KBUILD_LDFLAGS += -m elf_x86_64\nifdef CONFIG_LTO_CLANG_THIN\nKBUILD_LDFLAGS += existing\nendif\n";
        let r = kernel_text(src, 4).unwrap();
        assert!(r.contains("KBUILD_LDFLAGS += -m elf_x86_64"));
        assert!(r.contains("KBUILD_LDFLAGS += existing"));
        assert!(r.contains("--thinlto-jobs=4"));
        assert!(kernel_text(&r, 4).is_err());
        assert!(kernel_text("unknown", 4).is_err());
    }
    #[test] fn cgroup_parent_and_high_limit() {
        let root = env::temp_dir().join(format!("akt-cgroup-test-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("child")).unwrap();
        fs::write(root.join("memory.max"), (8 * GIB).to_string()).unwrap();
        fs::write(root.join("memory.current"), (2 * GIB).to_string()).unwrap();
        fs::write(root.join("child/memory.max"), "max").unwrap();
        assert_eq!(cgroup_memory(&root, "0::/child\n", 27 * GIB).unwrap(), 6 * GIB);
        fs::write(root.join("child/memory.high"), (3 * GIB).to_string()).unwrap();
        fs::write(root.join("child/memory.current"), GIB.to_string()).unwrap();
        assert_eq!(cgroup_memory(&root, "0::/child\n", 27 * GIB).unwrap(), 2 * GIB);
        assert!(cgroup_memory(&root, "0::/../escape\n", 27 * GIB).is_err());
        assert!(cgroup_memory(&root, "1:memory:/\n", 27 * GIB).is_err());
        fs::remove_dir_all(&root).unwrap(); // This test's exclusively created fixture only.
    }
}
