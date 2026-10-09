// The build engine. Orchestrates the documented linux-arxos flow for each host. Every external step
// is a shell command run with inherited stdio (live output). The config stage HARD-VERIFIES the
// ARXOS delta before the multi-hour compile, so a stripped option aborts here, not after hours.
use crate::{data_dir, gate, resources, Distro, Plan};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

// Shell snippets that tidy the output dir after a build: keep only the version just built and move
// any superseded older-version artifacts into prev/ (non-destructive rollback). makepkg / bindeb-pkg
// never prune, so without this every rebuild leaves the previous version behind — the clutter that
// makes `makepkg -e` refuse ("package group already built") and lets a stale older package be picked
// ahead of the new one. Mirrors the linux-arxos build.sh [4/4] tidy step.
const TIDY_ARCH: &str = r#"
NEWPKG=$(ls -t linux-$KNAME-*.pkg.tar.zst 2>/dev/null | grep -vE 'headers|gcc|debug' | head -1)
if [ -n "${NEWPKG:-}" ]; then
  b=$(basename "$NEWPKG"); CURVER=${b#linux-$KNAME-}; CURVER=${CURVER%%-x86_64*}
  mkdir -p prev; moved=0; shopt -s nullglob
  for f in linux-$KNAME*.pkg.tar.* config-*-$KNAME*; do
    case "$(basename "$f")" in
      *"$CURVER"*) : ;;
      *) mv -f "$f" prev/ 2>/dev/null && moved=$((moved+1)) || true ;;
    esac
  done
  shopt -u nullglob
  [ "$moved" -gt 0 ] && echo "  archived $moved superseded artifact(s) -> prev/ (roll back with: akt rollback)"
  echo "  current linux-$KNAME $CURVER:"; ls -1 linux-$KNAME*"$CURVER"*.pkg.tar.* 2>/dev/null | sed 's/^/    /'
else
  echo "  (no package produced — check log)"
fi
"#;

const TIDY_DEBIAN: &str = r#"
NEW=$(ls -t linux-image-*-$KNAME*.deb 2>/dev/null | head -1)
if [ -n "${NEW:-}" ]; then
  kr=$(basename "$NEW" | sed -E 's/^linux-image-([^_]+)_.*/\1/')
  mkdir -p prev; moved=0; shopt -s nullglob
  for f in linux-*-$KNAME*.deb; do
    case "$f" in *"$kr"*) : ;; *) mv -f "$f" prev/ 2>/dev/null && moved=$((moved+1)) || true ;; esac
  done
  shopt -u nullglob
  [ "$moved" -gt 0 ] && echo "  archived $moved superseded .deb(s) -> prev/ (rollback; prune freely)"
  echo "  current $kr:"; ls -1 *"$kr"*.deb 2>/dev/null | sed 's/^/    /'
else
  ls -1 *.deb 2>/dev/null || echo "  (none — check log)"
fi
"#;

fn print_plan(p: &Plan) {
    println!("== akt build: {} / flavor {} ==", p.distro.name(), p.flavor.key);
    println!("  base    : {}", p.base);
    for t in &p.tunes { println!("  tune    : {t}"); }
    for o in &p.overlays { println!("  overlay : {o}"); }
    for ps in &p.patch_sets { println!("  patches : {ps}"); }
    println!("  output  : {}\n", p.output);
}

/// Run a shell step with live output in `dir` with extra env. Returns an error on non-zero exit.
fn step(label: &str, dir: &Path, env: &[(&str, &str)], script: &str) -> io::Result<()> {
    println!(">> {label}");
    let mut c = Command::new("bash");
    c.arg("-c").arg(script).current_dir(dir);
    for (k, v) in env { c.env(k, v); }
    let status = c.status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, format!("step failed ({label})")));
    }
    Ok(())
}

/// A cross-process advisory build lock (flock) held for the lifetime of the returned guard, and
/// released automatically when the process exits or the guard drops, even on crash. Shared by the
/// CLI and the GUI (both go through build_arch/build_debian), so two builds can never mutate the same
/// work dir at once (makepkg -C / rm -rf would otherwise corrupt an in-flight tree). A per-process
/// Option<Child> could not cover a second process; this can.
///
/// The lock fd's close-on-exec flag is CLEARED so the build subprocesses this worker spawns (makepkg)
/// inherit the same open file description. The flock is tied to that description, so it stays held as
/// long as ANY holder keeps the fd open. If the worker is killed mid-build while makepkg survives, the
/// lock therefore stays held by makepkg and a second build cannot start until the real mutation ends.
struct BuildLock(#[allow(dead_code)] std::fs::File);
extern "C" { fn flock(fd: i32, op: i32) -> i32; fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32; }

/// Preserve other descriptor flags and fail closed if inheritance cannot be enabled.
fn inherit_build_lock(fd: std::os::unix::io::RawFd) -> io::Result<()> {
    const F_GETFD: i32 = 1;
    const F_SETFD: i32 = 2;
    const FD_CLOEXEC: i32 = 1;
    let flags = unsafe { fcntl(fd, F_GETFD, 0) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { fcntl(fd, F_SETFD, flags & !FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn acquire_build_lock(work: &Path) -> io::Result<BuildLock> {
    use std::os::unix::io::AsRawFd;
    let f = std::fs::OpenOptions::new().write(true).create(true).truncate(false)
        .open(work.join(".akt-build.lock"))?;
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    if unsafe { flock(f.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
        return Err(io::Error::new(io::ErrorKind::WouldBlock,
            format!("another akt build is already running in {} (build lock held)", work.display())));
    }
    // Do not start a build unless children can retain the lock after worker death.
    // On failure, returning drops f and releases the lock before any recipe mutation.
    inherit_build_lock(f.as_raw_fd())?;
    Ok(BuildLock(f))
}

/// Resolve a user-writable build workspace separately from the root-owned installed data.
/// AKT_WORK remains the explicit override; otherwise use the user's XDG cache (or ~/.cache).
pub fn work_dir(distro: Distro, flavor: &str) -> io::Result<PathBuf> {
    if let Some(work) = std::env::var_os("AKT_WORK").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(work));
    }
    let cache_root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|home| home.join(".cache"))
        })
        .ok_or_else(|| io::Error::new(
            io::ErrorKind::NotFound,
            "cannot determine a cache directory; set AKT_WORK or HOME/XDG_CACHE_HOME",
        ))?;
    Ok(cache_root.join("akt").join("build")
        .join(format!("{}-{flavor}", distro.name())))
}

pub fn run(plan: Plan, dry: bool) -> io::Result<()> {
    if plan.distro == Distro::Unknown {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "unsupported host (need Arch- or Debian-family)"));
    }
    print_plan(&plan);
    if dry {
        let work = work_dir(plan.distro, plan.flavor.key)?;
        resources::detect(&work)?.report();
        println!("[dry-run] no build performed. Re-run without --dry-run to build.");
        return Ok(());
    }
    match plan.distro {
        Distro::Arch => build_arch(&plan),
        Distro::Debian => build_debian(&plan),
        Distro::Unknown => unreachable!(),
    }
}

fn cpu_target_level(target: &str, work: &Path) -> io::Result<u8> {
    let level = if target == "auto" {
        crate::sysinfo::SystemInfo::detect(work).x86_level.max(1)
    } else if target == "portable" || target == "v1" { 1 }
    else { target.strip_prefix('v').and_then(|v| v.parse::<u8>().ok()).unwrap_or(1) };
    if !(1..=4).contains(&level) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "CPU target must resolve to x86-64-v1 through v4"));
    }
    Ok(level)
}

fn cpu_processor_opt(target: &str, work: &Path) -> io::Result<String> {
    Ok(format!("GENERIC_V{}", cpu_target_level(target, work)?))
}

// Upstream linux-cachyos PKGBUILD revision 59f65f1 adds scheduler patches to
// \`source\` conditionally but omits their checksums from the generic recipe's \`b2sums\`.
// These hashes are copied from CachyOS's matching variant .SRCINFO files for the SAME
// recipe commit. Keep this narrowly pinned: if the recipe or patch changes, fail closed.
#[derive(Default)]
struct SrcInfo {
    pkgver: Option<String>,
    pkgrel: Option<String>,
    sources: Vec<String>,
    b2sums: Vec<String>,
}

fn parse_srcinfo(text: &str) -> SrcInfo {
    let mut info = SrcInfo::default();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else { continue };
        let key = key.trim();
        let value = value.trim().to_string();
        match key {
            "pkgver" if info.pkgver.is_none() => info.pkgver = Some(value),
            "pkgrel" if info.pkgrel.is_none() => info.pkgrel = Some(value),
            "source" | "source_x86_64" => info.sources.push(value),
            "b2sums" | "b2sums_x86_64" => info.b2sums.push(value),
            _ => {}
        }
    }
    info
}

fn source_key(source: &str) -> &str {
    source.split_once("::").map(|(url, _)| url).unwrap_or(source)
}

fn valid_b2sum(value: &str) -> bool {
    value == "SKIP" || (value.len() == 128 && value.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Lookup a source's BLAKE2 checksum from a generated .SRCINFO belonging to the exact same
/// CachyOS package version. This deliberately reads version-controlled metadata in the checked-out
/// recipe commit; it never trusts a newly downloaded patch's own digest as its expected checksum.
fn checksum_in_srcinfo(
    text: &str,
    expected_pkgver: &str,
    expected_pkgrel: &str,
    requested_source: &str,
) -> Option<String> {
    let info = parse_srcinfo(text);
    if info.pkgver.as_deref() != Some(expected_pkgver)
        || info.pkgrel.as_deref() != Some(expected_pkgrel)
        || info.sources.len() != info.b2sums.len()
    {
        return None;
    }
    let wanted = source_key(requested_source);
    let mut found = info.sources.iter()
        .zip(info.b2sums.iter())
        .filter(|(source, _)| source_key(source) == wanted)
        .map(|(_, sum)| sum.clone());
    let first = found.next()?;
    if !valid_b2sum(&first) || found.any(|other| other != first) {
        return None;
    }
    Some(first)
}

fn srcinfo_checksum(
    repo_root: &Path,
    base_srcinfo: &Path,
    pkgver: &str,
    pkgrel: &str,
    source: &str,
) -> io::Result<String> {
    // Prefer the current generic PKGBUILD's own metadata when it defines this source (especially
    // the variant-specific local "config" file). Scheduler-only URLs are resolved from sibling
    // variant .SRCINFO files from the same repository commit.
    if let Ok(contents) = std::fs::read_to_string(base_srcinfo) {
        if let Some(sum) = checksum_in_srcinfo(&contents, pkgver, pkgrel, source) {
            return Ok(sum);
        }
    }

    let entries = std::fs::read_dir(repo_root)?;
    let mut matches = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
        let metadata = entry.path().join(".SRCINFO");
        let Ok(contents) = std::fs::read_to_string(&metadata) else { continue };
        if let Some(sum) = checksum_in_srcinfo(&contents, pkgver, pkgrel, source) {
            matches.push(sum);
        }
    }
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [sum] => Ok(sum.clone()),
        [] => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "no matching source checksum found in CachyOS .SRCINFO metadata for {source} (package {pkgver}-{pkgrel}); refusing to skip integrity checks"
            ),
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("CachyOS .SRCINFO files disagree on the checksum for {source}; refusing to choose one"),
        )),
    }
}

fn active_srcinfo(pkgbuild_dir: &Path, env: &[(&str, &str)]) -> io::Result<SrcInfo> {
    let mut command = Command::new("makepkg");
    command.arg("--printsrcinfo").current_dir(pkgbuild_dir);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "makepkg --printsrcinfo failed while resolving active source/checksum inputs: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let info = parse_srcinfo(&String::from_utf8_lossy(&output.stdout));
    if info.sources.is_empty() || info.pkgver.as_deref().unwrap_or("").is_empty()
        || info.pkgrel.as_deref().unwrap_or("").is_empty()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "makepkg --printsrcinfo did not provide package version, release, and source entries",
        ));
    }
    Ok(info)
}

fn replace_b2sums_array(original: &str, sums: &[String]) -> io::Result<String> {
    let start = original.find("b2sums=(").ok_or_else(|| io::Error::new(
        io::ErrorKind::InvalidData, "CachyOS PKGBUILD has no b2sums array",
    ))?;
    if original.contains("b2sums+=(") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "CachyOS PKGBUILD uses an additional b2sums+= array; refusing to rewrite unknown checksum logic",
        ));
    }
    let array_end = original[start..].find(')').map(|n| start + n)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unterminated b2sums array"))?;
    let mut replacement = String::from("b2sums=(\n");
    for sum in sums {
        if !valid_b2sum(sum) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid checksum in selected CachyOS metadata"));
        }
        replacement.push_str("    '");
        replacement.push_str(sum);
        replacement.push_str("'\n");
    }
    replacement.push(')');
    let mut updated = String::with_capacity(original.len() + replacement.len());
    updated.push_str(&original[..start]);
    updated.push_str(&replacement);
    updated.push_str(&original[array_end + 1..]);
    Ok(updated)
}

/// Rebuild b2sums directly from the source URLs resolved by makepkg and the versioned .SRCINFO
/// metadata shipped alongside this exact CachyOS recipe commit. This automatically follows upstream
/// source and checksum changes while preserving makepkg's verification; unknown sources fail closed.
fn sync_cachyos_b2sums(pkgbuild: &Path, repo_root: &Path, env: &[(&str, &str)]) -> io::Result<()> {
    let original = std::fs::read_to_string(pkgbuild)?;
    let active = active_srcinfo(pkgbuild.parent().unwrap_or(repo_root), env)?;
    let pkgver = active.pkgver.as_deref().unwrap_or("");
    let pkgrel = active.pkgrel.as_deref().unwrap_or("");
    let mut sums = Vec::with_capacity(active.sources.len());
    for source in &active.sources {
        sums.push(srcinfo_checksum(
            repo_root,
            &pkgbuild.parent().unwrap_or(repo_root).join(".SRCINFO"),
            pkgver,
            pkgrel,
            source,
        )?);
    }
    if active.sources.len() != sums.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "source/checksum count mismatch after CachyOS metadata sync"));
    }
    let updated = replace_b2sums_array(&original, &sums)?;
    if updated != original {
        std::fs::write(pkgbuild, updated)?;
        println!(
            "akt: synchronized {} BLAKE2 entries from matching CachyOS .SRCINFO metadata; makepkg integrity verification remains enabled.",
            sums.len()
        );
    } else {
        println!("akt: CachyOS BLAKE2 metadata already matches the checked-out source revision.");
    }
    Ok(())
}

fn verify_tuning_config(config: &Path, p: &Plan, cpu_level: u8, check_cpu_level: bool) -> io::Result<()> {
    let text = std::fs::read_to_string(config)?;
    let has = |line: &str| text.lines().any(|v| v.trim() == line);
    let fail = |what: &str| io::Error::new(io::ErrorKind::InvalidData,
        format!("requested tune did not survive Kconfig resolution: {what}; refusing to compile"));
    if !has(&format!("CONFIG_HZ={}", p.hz)) {
        return Err(fail(&format!("CONFIG_HZ={}", p.hz)));
    }
    // PREEMPT_RT kernels depend on !TRANSPARENT_HUGEPAGE, so THP policy does not apply there.
    if p.flavor.key != "rt" {
        let thp = format!("CONFIG_TRANSPARENT_HUGEPAGE_{}=y", p.hugepages.to_ascii_uppercase());
        if !has(&thp) { return Err(fail(&thp)); }
    }
    if p.performance_governor && !has("CONFIG_CPU_FREQ_DEFAULT_GOV_PERFORMANCE=y") {
        return Err(fail("CONFIG_CPU_FREQ_DEFAULT_GOV_PERFORMANCE=y"));
    }
    if check_cpu_level && !has(&format!("CONFIG_X86_64_VERSION={cpu_level}")) {
        return Err(fail(&format!("CONFIG_X86_64_VERSION={cpu_level}")));
    }
    if p.flavor.key == "rt" && !has("CONFIG_PREEMPT_RT=y") {
        return Err(fail("CONFIG_PREEMPT_RT=y"));
    }
    if p.flavor.lto == "thin" && !has("CONFIG_LTO_CLANG_THIN=y") {
        return Err(fail("CONFIG_LTO_CLANG_THIN=y"));
    }
    if p.flavor.key == "hardened" {
        if !has("CONFIG_CFI_CLANG=y") { return Err(fail("CONFIG_CFI_CLANG=y (hardened/kCFI profile)")); }
        if !has("CONFIG_CFI_AUTO_DEFAULT=y") { return Err(fail("CONFIG_CFI_AUTO_DEFAULT=y (hardened/kCFI profile)")); }
    }
    if p.distro == Distro::Arch && matches!(p.flavor.sched, "bore" | "rt-bore" | "hardened")
        && !has("CONFIG_SCHED_BORE=y") {
        return Err(fail("CONFIG_SCHED_BORE=y (selected CachyOS scheduler)"));
    }
    if p.distro == Distro::Debian && text.contains("CONFIG_SCHED_BORE")
        && !has("CONFIG_SCHED_BORE=y") {
        return Err(fail("CONFIG_SCHED_BORE=y (source includes BORE support)"));
    }
    Ok(())
}

fn build_arch(p: &Plan) -> io::Result<()> {
    let dd = data_dir();
    let work = work_dir(Distro::Arch, p.flavor.key)?.display().to_string();
    std::fs::create_dir_all(&work)?;
    let workp = Path::new(&work);
    let _lock = acquire_build_lock(workp)?;   // cross-process (CLI+GUI) lock; held until this returns
    let frag = dd.join("config/arxos.fragment");
    let initial = resources::detect(workp)?;
    initial.report();
    let jobs = initial.jobs.to_string();

    // 1. fetch the CachyOS recipe. A specified source ref is checked out detached and never pulled forward.
    let requested_ref = p.source_ref.as_deref().unwrap_or("");
    step("[1/5] fetch CachyOS linux-cachyos", workp, &[("ARXOS_SOURCE_REF", requested_ref)],
        "set -e; if [ ! -d cos/.git ]; then git clone -q https://github.com/CachyOS/linux-cachyos.git cos; fi; \
         git -C cos checkout -- linux-cachyos/PKGBUILD; \
         if [ -n \"$ARXOS_SOURCE_REF\" ]; then \
           git -C cos fetch -q --tags origin \"$ARXOS_SOURCE_REF\"; \
           git -C cos checkout -q --detach FETCH_HEAD; \
         else git -C cos pull -q --ff-only || true; fi")?;

    let base = format!("{work}/cos/linux-cachyos");
    let basep = Path::new(&base);
    let revision = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(basep).output()?;
    if !revision.status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "cannot identify the CachyOS recipe revision"));
    }
    let source_revision = String::from_utf8_lossy(&revision.stdout).trim().to_owned();
    println!("source revision: {source_revision}");
    println!("source selection: {}", if p.source_ref.is_some() { p.source_ref.as_deref().unwrap_or("pinned") } else { "rolling latest" });
    let cpu_opt = cpu_processor_opt(&p.cpu_target, workp)?;
    let pgov = if p.performance_governor { "yes" } else { "no" };
    let kcfi = if p.flavor.key == "hardened" { "yes" } else { "no" };
    let env = [
        ("_HZ_ticks", p.hz.as_str()), ("_tcp_bbr3", "yes"), ("_processor_opt", cpu_opt.as_str()),
        ("_hugepage", p.hugepages.as_str()), ("_per_gov", pgov), ("_use_kcfi", kcfi),
        ("_cpusched", p.flavor.sched), ("_use_llvm_lto", p.flavor.lto), ("_preempt", "full"),
        ("_build_zfs", "no"), ("_build_nvidia_open", "no"), ("_cachy_config", "yes"),
        ("PACKAGER", "ARXOS <team@arxos.uk>"), ("ARXOS_PKGREL", p.pkgrel.as_str()), ("KNAME", p.kname.as_str()),
        ("ARXOS_BUILD_JOBS", jobs.as_str()),
    ];
    resources::limit_recipe(&basep.join("PKGBUILD"))?;
    sync_cachyos_b2sums(&basep.join("PKGBUILD"), basep.parent().unwrap_or(basep), &env)?;

    // 2. de-brand + pkgrel (matches linux-arxos build.sh).
    step(&format!("[2/5] de-brand recipe -> linux-{}", p.kname), basep, &env,
        "sed -i \"s/_pkgsuffix=cachyos/_pkgsuffix=$KNAME/g; s/KBUILD_BUILD_HOST=cachyos/KBUILD_BUILD_HOST=$KNAME/\" PKGBUILD; \
         sed -i \"s/^pkgrel=.*/pkgrel=${ARXOS_PKGREL:-1}/\" PKGBUILD")?;

    // 3. extract + prepare. NO --skipinteg: makepkg verifies the recipe's source checksums against
    //    what it downloaded (fail-closed on a tampered/mismatched source), matching the private
    //    linux-arxos builder. `-C` cleans the previous src/pkg for this recipe only.
    step("[3/5] extract + prepare", basep, &env, "makepkg -o -C --nodeps --noconfirm")?;

    // 4. overlay ARXOS config + patches, RE-ASSERT + HARD-VERIFY the delta (fail-fast).
    let kdir = find_kdir(basep).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "kernel source dir not found under src/"))?;
    let (pfiles, pdirs) = collect_patches(&bundled_patch_dirs(p.distro), &p.extra_patches);
    if !pfiles.is_empty() {
        if !pdirs.is_empty() { step("verify patch integrity", Path::new(&kdir), &[], &integrity_script(&pdirs))?; }
        step(&format!("apply {} patch(es) [{}]", pfiles.len(), if p.patch_strict { "strict" } else { "optional" }),
            Path::new(&kdir), &[], &apply_patches_script(&pfiles, p.patch_strict))?;
    }
    // Pass the fragment path via a quoted env var (never interpolated into the shell) so a data_dir
    // with spaces/metacharacters is safe.
    let frag_str = frag.display().to_string();
    let mut env4 = env.to_vec();
    env4.push(("ARXOS_FRAG", frag_str.as_str()));
    step("[4/5] overlay ARXOS config + verify hardening delta", Path::new(&kdir), &env4,
        "./scripts/kconfig/merge_config.sh -m .config \"$ARXOS_FRAG\"; ./scripts/config -d RUST; make LLVM=1 olddefconfig; \
         ./scripts/config --enable INIT_ON_FREE_DEFAULT_ON --enable INIT_ON_ALLOC_DEFAULT_ON --disable HIBERNATION \
           --enable INTEL_IOMMU_DEFAULT_ON --enable IOMMU_DEFAULT_DMA_STRICT --enable SECURITY_LOCKDOWN_LSM_EARLY \
           --enable INTEGRITY --enable IMA --enable IMA_DEFAULT_HASH_SHA256 --enable SECURITY_DMESG_RESTRICT \
           --enable LIVEPATCH; make LLVM=1 olddefconfig")?;
    if !gate::verify(&format!("{kdir}/.config"), false) {
        return Err(io::Error::new(io::ErrorKind::Other, "ARXOS hardening delta missing after config — aborting before compile"));
    }
    let selected_level = cpu_target_level(&p.cpu_target, workp)?;
    verify_tuning_config(Path::new(&format!("{kdir}/.config")), p, selected_level, true)?;
    let detected_level = crate::sysinfo::SystemInfo::detect(workp).x86_level;
    if selected_level > detected_level && p.cpu_target != "portable" {
        eprintln!("WARNING: requested x86-64-v{selected_level} exceeds locally detected baseline v{detected_level}; the kernel may not boot on this machine.");
    }

    // 5. build (-f overwrites a stale same-version pkg from a prior, e.g. wrong-config, build).
    let current = resources::detect(workp)?;
    current.report();
    resources::limit_kernel(Path::new(&kdir), current.link_jobs)?;
    let jobs = current.jobs.to_string();
    let mut compile_env = env.to_vec();
    compile_env.push(("ARXOS_BUILD_JOBS", jobs.as_str()));
    step("[5/5] compile (ThinLTO — long)", basep, &compile_env, "makepkg -e -f --nodeps --noconfirm")?;
    // tidy the output dir: keep only what we just built, archive superseded versions into prev/.
    println!("\nDone.");
    let _ = step("tidy output dir", basep, &[("KNAME", p.kname.as_str())], TIDY_ARCH);
    if let Some(dir) = &p.out {
        step(&format!("move packages -> {dir}"), basep, &[("OUT", dir.as_str()), ("KNAME", p.kname.as_str())],
            "mkdir -p \"$OUT\" && mv -f linux-$KNAME*.pkg.tar.* \"$OUT\"/ 2>/dev/null && echo \"  moved to $OUT\" || echo \"  (nothing to move)\"")?;
    }
    println!("Install with:  sudo pacman -U <pkg>   (verify + boot-test in a throwaway VM first)");
    println!("Rollback:      `akt rollback --name {}` reinstalls the previous package from prev/.", p.kname);
    println!("               (A same-name `pacman -U` upgrade REMOVES the old version's /usr/lib/modules,");
    println!("               so keeping only the old vmlinuz+initramfs is NOT enough to roll back — the");
    println!("               retained prev/ package is. For a permanent second kernel, build with --name.)");
    Ok(())
}

/// Read pkgname + pkgver from a package's .PKGINFO. argv-based (path passed as an OS arg, never
/// through a shell) so spaces/metacharacters in the path are safe. Returns None if not a readable
/// package. Identity comes from metadata, NOT the filename.
fn pkg_identity(pkg: &Path) -> Option<(String, String)> {
    let out = Command::new("bsdtar").arg("-xOf").arg(pkg).arg(".PKGINFO").output().ok()?;
    if !out.status.success() { return None; }
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut name, mut ver) = (None, None);
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("pkgname = ") { name = Some(v.trim().to_string()); }
        else if let Some(v) = line.strip_prefix("pkgver = ") { ver = Some(v.trim().to_string()); }
    }
    match (name, ver) { (Some(n), Some(v)) => Some((n, v)), _ => None }
}

/// Compare two pkgvers using pacman's own `vercmp` (the only correct kernel-version ordering, e.g.
/// 7.9 < 7.10 where a byte compare would disagree). FAIL-CLOSED: if vercmp is missing, exits non-zero,
/// or prints anything but -1/0/1, return an error rather than guessing — a wrong comparison here could
/// install the wrong kernel. Never falls back to a byte compare.
fn vercmp(a: &str, b: &str) -> io::Result<std::cmp::Ordering> {
    use std::cmp::Ordering;
    let out = match Command::new("vercmp").arg(a).arg(b).output() {
        Ok(out) => out,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(fallback_vercmp(a, b)),
        Err(e) => return Err(io::Error::new(io::ErrorKind::Other, format!("could not run vercmp: {e}"))),
    };
    if !out.status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, format!("vercmp {a} {b} exited non-zero")));
    }
    match String::from_utf8_lossy(&out.stdout).trim() {
        "-1" => Ok(Ordering::Less),
        "0"  => Ok(Ordering::Equal),
        "1"  => Ok(Ordering::Greater),
        other => Err(io::Error::new(io::ErrorKind::Other,
            format!("vercmp gave unparseable output {other:?} for {a} vs {b}"))),
    }
}

/// Compare Arch versions via pacman when present, using a conservative fallback on non-Arch build hosts.
/// Conservative libalpm-style version comparison fallback for build hosts without pacman's `vercmp` binary.
/// Native Arch hosts still use the exact pacman implementation whenever available.
fn version_segment_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let aa = a.as_bytes();
    let bb = b.as_bytes();
    let (mut i, mut j) = (0usize, 0usize);
    loop {
        while i < aa.len() && !aa[i].is_ascii_alphanumeric() && aa[i] != b'~' { i += 1; }
        while j < bb.len() && !bb[j].is_ascii_alphanumeric() && bb[j] != b'~' { j += 1; }
        if i < aa.len() && aa[i] == b'~' || j < bb.len() && bb[j] == b'~' {
            match (aa.get(i) == Some(&b'~'), bb.get(j) == Some(&b'~')) {
                (true, true) => { i += 1; j += 1; continue; }
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                (false, false) => unreachable!(),
            }
        }
        if i >= aa.len() || j >= bb.len() { break; }
        let an = aa[i].is_ascii_digit();
        let bn = bb[j].is_ascii_digit();
        if an != bn { return if an { Ordering::Greater } else { Ordering::Less }; }
        let (start_a, start_b, end_a, end_b) = if an {
            let mut ea = i;
            let mut eb = j;
            while ea < aa.len() && aa[ea].is_ascii_digit() { ea += 1; }
            while eb < bb.len() && bb[eb].is_ascii_digit() { eb += 1; }
            let mut sa = i;
            let mut sb = j;
            while sa < ea && aa[sa] == b'0' { sa += 1; }
            while sb < eb && bb[sb] == b'0' { sb += 1; }
            (sa, sb, ea, eb)
        } else {
            let mut ea = i;
            let mut eb = j;
            while ea < aa.len() && aa[ea].is_ascii_alphabetic() { ea += 1; }
            while eb < bb.len() && bb[eb].is_ascii_alphabetic() { eb += 1; }
            (i, j, ea, eb)
        };
        let lhs = &aa[start_a..end_a];
        let rhs = &bb[start_b..end_b];
        if an && lhs.len() != rhs.len() {
            return lhs.len().cmp(&rhs.len());
        }
        match lhs.cmp(rhs) {
            Ordering::Equal => {}
            other => return other,
        }
        i = end_a;
        j = end_b;
    }
    while i < aa.len() && !aa[i].is_ascii_alphanumeric() && aa[i] != b'~' { i += 1; }
    while j < bb.len() && !bb[j].is_ascii_alphanumeric() && bb[j] != b'~' { j += 1; }
    match (i >= aa.len(), j >= bb.len()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => Ordering::Equal,
    }
}

fn fallback_vercmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn epoch_and_version(s: &str) -> (u64, &str) {
        if let Some((epoch, rest)) = s.split_once(':') {
            if !epoch.is_empty() && epoch.bytes().all(|b| b.is_ascii_digit()) {
                return (epoch.parse().unwrap_or(0), rest);
            }
        }
        (0, s)
    }
    let (ae, av) = epoch_and_version(a);
    let (be, bv) = epoch_and_version(b);
    match ae.cmp(&be) {
        Ordering::Equal => {}
        other => return other,
    }
    let (av, ar) = bv_parts(av);
    let (bv, br) = bv_parts(bv);
    match version_segment_cmp(av, bv) {
        Ordering::Equal => version_segment_cmp(ar.unwrap_or("0"), br.unwrap_or("0")),
        other => other,
    }
}
fn bv_parts(s: &str) -> (&str, Option<&str>) {
    match s.rsplit_once('-') {
        Some((version, release)) => (version, Some(release)),
        None => (s, None),
    }
}

/// The installed pkgver of a package (via `pacman -Q <pkg>`), or None if it is not installed.
fn installed_pkgver(pkg: &str) -> Option<String> {
    let out = Command::new("pacman").arg("-Q").arg(pkg).output().ok()?;
    if !out.status.success() { return None; }
    String::from_utf8_lossy(&out.stdout).split_whitespace().nth(1).map(|v| v.to_string())
}

/// Roll back to the previously-built kernel: reinstall the retained linux-<kname> package archived in
/// prev/. This is the mechanism the ArxOS kernel gate proved is REQUIRED — a same-name pacman upgrade
/// deletes the prior version's /usr/lib/modules, so a saved kernel image alone cannot boot cleanly;
/// reinstalling the retained package restores the module tree + rebuilds the initramfs via pacman hooks.
///
/// Selection is done in Rust (no shell): scan prev/, accept only real package archives (exact
/// .pkg.tar.{zst,xz,gz,} extensions — never .sig or other suffixes), read each one's .PKGINFO for its
/// true pkgname/pkgver, pick the highest-version linux-<kname> by pacman vercmp, and pair it with the
/// linux-<kname>-headers package OF THE SAME VERSION. pacman -U is invoked argv-based via sudo.
pub fn rollback(p: &Plan) -> io::Result<()> {
    if p.distro != Distro::Arch {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "akt rollback currently supports the Arch path only"));
    }
    let work = work_dir(Distro::Arch, p.flavor.key)?.display().to_string();
    let prev = Path::new(&work).join("cos/linux-cachyos/prev");
    let kpkg = format!("linux-{}", p.kname);
    let hpkg = format!("linux-{}-headers", p.kname);
    let is_pkg = |n: &str| n.ends_with(".pkg.tar.zst") || n.ends_with(".pkg.tar.xz")
                        || n.ends_with(".pkg.tar.gz") || n.ends_with(".pkg.tar");

    let rd = std::fs::read_dir(&prev).map_err(|_| {
        io::Error::new(io::ErrorKind::NotFound,
            format!("no archived packages in {} (nothing to roll back to)", prev.display()))
    })?;
    let mut kernels: Vec<(String, std::path::PathBuf)> = vec![]; // (pkgver, path)
    let mut headers: Vec<(String, std::path::PathBuf)> = vec![];
    for e in rd.flatten() {
        let path = e.path();
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) { continue; }
        if !is_pkg(&e.file_name().to_string_lossy()) { continue; }        // reject .sig/other suffixes
        let Some((name, ver)) = pkg_identity(&path) else { continue };    // identity from metadata
        if name == kpkg { kernels.push((ver, path)); }
        else if name == hpkg { headers.push((ver, path)); }
    }
    if kernels.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound,
            format!("no archived {} package in {} (nothing to roll back to)", kpkg, prev.display())));
    }
    // Pick the highest-version archived kernel, failing closed if any pair can't be compared.
    let mut best = kernels[0].clone();
    for cand in &kernels[1..] {
        if vercmp(&cand.0, &best.0)? == std::cmp::Ordering::Greater { best = cand.clone(); }
    }
    let (kver, kpath) = best;

    // Refuse a non-downgrade: rollback must go to a version OLDER than what is installed, so the
    // highest prev/ entry can never silently UPGRADE the running kernel. (If the package is not
    // installed at all, there is nothing to be older than — install the archived one as-is.)
    if let Some(iv) = installed_pkgver(&kpkg) {
        match vercmp(&kver, &iv)? {
            std::cmp::Ordering::Less => {}
            _ => return Err(io::Error::new(io::ErrorKind::Other, format!(
                "archived {kpkg} {kver} is not older than the installed {iv}; refusing to roll back \
                 to a same-or-newer version (build + install it explicitly if that is what you want)"))),
        }
        println!("rolling back {} {} -> {} (downgrade)", kpkg, iv, kver);
    } else {
        println!("installing archived {} {} ({} is not currently installed)", kpkg, kver, kpkg);
    }
    let hmatch = headers.iter().find(|(hv, _)| *hv == kver).map(|(_, hp)| hp.clone());
    println!("  kernel : {}", kpath.display());
    // argv-based, no shell; `--` ends option parsing so a path starting with '-' is never a flag.
    let mut args: Vec<std::ffi::OsString> =
        vec!["pacman".into(), "-U".into(), "--noconfirm".into(), "--".into(), kpath.clone().into()];
    match &hmatch {
        Some(hp) => { println!("  headers: {}", hp.display()); args.push(hp.clone().into()); }
        None => println!("  headers: none matching version {} (installing kernel only)", kver),
    }
    let status = Command::new("sudo").args(&args).status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "pacman -U failed"));
    }
    println!("installed {} {}. Reboot and select this kernel; boot-test in a throwaway VM first.", kpkg, kver);
    Ok(())
}

fn build_debian(p: &Plan) -> io::Result<()> {
    let dd = data_dir();
    let work = work_dir(Distro::Debian, p.flavor.key)?.display().to_string();
    std::fs::create_dir_all(&work)?;
    let workp = Path::new(&work);
    let _lock = acquire_build_lock(workp)?;   // cross-process (CLI+GUI) lock; held until this returns
    let frag = dd.join("config/arxos.fragment");
    resources::detect(workp)?.report();

    // 1. Debian kernel source. `linux-source` (from the distro) is the reproducible route; the git
    //    tree is the alternative. We take the distro source so the .deb tooling matches the host.
    step("[1/6] fetch Debian kernel source", workp, &[],
        "if ! ls linux-source-*/ >/dev/null 2>&1; then \
           sudo apt-get -y install linux-source build-essential fakeroot libncurses-dev bison flex libssl-dev libelf-dev clang llvm lld bc dwarves sudo; \
           tar xaf /usr/src/linux-source-*.tar.* ; fi")?;
    // find the extracted source dir
    let src = first_dir(workp, "linux-source-").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound,
        "linux-source-*/ not found — install the `linux-source` package for your kernel"))?;
    let srcp = Path::new(&src);
    println!("Debian kernel source tree: {}", srcp.display());

    // 2. start from the running kernel's config, then overlay CachyOS-style tunes + the ARXOS delta.
    step("[2/6] seed config from the running kernel", srcp, &[],
        "cp -f /boot/config-$(uname -r) .config 2>/dev/null || make defconfig")?;
    let cpu_level = cpu_target_level(&p.cpu_target, workp)?;
    let pgov = if p.performance_governor { "yes" } else { "no" };
    let flavor_key = p.flavor.key;
    let thp = p.hugepages.as_str();
    let hz = p.hz.as_str();
    let cpu_level_s = cpu_level.to_string();
    let tune_env = [
        ("ARXOS_CPU_LEVEL", cpu_level_s.as_str()), ("ARXOS_HZ", hz), ("ARXOS_THP", thp),
        ("ARXOS_PGOV", pgov), ("ARXOS_FLAVOR", flavor_key), ("ARXOS_LTO", p.flavor.lto),
    ];
    step("[3/6] apply available performance Kconfig options", srcp, &tune_env,
        "set -e; \
         ./scripts/config --set-val HZ \"$ARXOS_HZ\" --enable \"HZ_$ARXOS_HZ\" --enable TCP_CONG_BBR --set-str DEFAULT_TCP_CONG bbr \
           --enable TRANSPARENT_HUGEPAGE --enable \"TRANSPARENT_HUGEPAGE_$(printf %s \"$ARXOS_THP\" | tr '[:lower:]' '[:upper:]')\" \
           --enable LRU_GEN --enable LRU_GEN_ENABLED; \
         if grep -Rqs 'config SCHED_BORE' kernel/sched init; then ./scripts/config --enable SCHED_BORE; else echo 'NOTICE: Debian source has no BORE patch; upstream scheduler remains enabled.'; fi; \
         if grep -Rqs 'config X86_64_VERSION' arch/x86; then ./scripts/config --enable GENERIC_CPU --set-val X86_64_VERSION \"$ARXOS_CPU_LEVEL\"; else echo 'ERROR: source cannot set an x86-64 CPU baseline' >&2; exit 7; fi; \
         if [ \"$ARXOS_PGOV\" = yes ]; then ./scripts/config --enable CPU_FREQ_DEFAULT_GOV_PERFORMANCE; fi; \
         if [ \"$ARXOS_LTO\" = thin ]; then ./scripts/config --enable LTO_CLANG_THIN; else ./scripts/config --disable LTO_CLANG_THIN --disable LTO_CLANG_FULL; fi; \
         if [ \"$ARXOS_FLAVOR\" = rt ]; then ./scripts/config --enable PREEMPT_RT --disable LTO_CLANG_THIN --disable LTO_CLANG_FULL; fi; \
         if [ \"$ARXOS_FLAVOR\" = hardened ]; then \
           if grep -q '^CONFIG_ARCH_SUPPORTS_CFI_CLANG=y' .config; then ./scripts/config --enable CFI_CLANG --enable CFI_AUTO_DEFAULT; \
           else echo 'ERROR: hardened flavor requested kCFI, but this source config does not support it' >&2; exit 8; fi; \
         fi; \
         make LLVM=1 olddefconfig")?;

    // 3. overlay the ARXOS hardening delta + Kali/ARXOS patches, then verify.
    let (pfiles, pdirs) = collect_patches(&bundled_patch_dirs(p.distro), &p.extra_patches);
    if !pfiles.is_empty() {
        if !pdirs.is_empty() { step("verify patch integrity", srcp, &[], &integrity_script(&pdirs))?; }
        step(&format!("apply {} patch(es) [{}]", pfiles.len(), if p.patch_strict { "strict" } else { "optional" }),
            srcp, &[], &apply_patches_script(&pfiles, p.patch_strict))?;
    }
    let frag_str = frag.display().to_string();   // quoted env var, not shell-interpolated (path-safe)
    step("[4/6] overlay ARXOS config + verify hardening delta", srcp, &[("ARXOS_FRAG", frag_str.as_str())],
        "set -e; ./scripts/kconfig/merge_config.sh -m .config \"$ARXOS_FRAG\"; make LLVM=1 olddefconfig; \
         ./scripts/config --enable INIT_ON_FREE_DEFAULT_ON --enable INIT_ON_ALLOC_DEFAULT_ON --disable HIBERNATION \
           --enable INTEL_IOMMU_DEFAULT_ON --enable IOMMU_DEFAULT_DMA_STRICT --enable SECURITY_LOCKDOWN_LSM_EARLY \
           --enable INTEGRITY --enable IMA --enable IMA_DEFAULT_HASH_SHA256 --enable SECURITY_DMESG_RESTRICT \
           --enable LIVEPATCH; make LLVM=1 olddefconfig")?;
    if !gate::verify(&format!("{src}/.config"), false) {
        return Err(io::Error::new(io::ErrorKind::Other, "ARXOS hardening delta missing after config — aborting before compile"));
    }
    verify_tuning_config(Path::new(&format!("{src}/.config")), p, cpu_level, true)?;

    // 4. build .deb packages.
    let current = resources::detect(workp)?;
    current.report();
    resources::limit_kernel(srcp, current.link_jobs)?;
    let jobs = current.jobs.to_string();
    let make = if p.flavor.lto == "thin" { "make LLVM=1 -j\"$ARXOS_BUILD_JOBS\" LOCALVERSION=-$KNAME bindeb-pkg" } else { "make -j\"$ARXOS_BUILD_JOBS\" LOCALVERSION=-$KNAME bindeb-pkg" };
    step("[5/6] build .deb (long)", srcp, &[("KNAME", p.kname.as_str()), ("ARXOS_BUILD_JOBS", jobs.as_str())], make)?;
    // tidy: keep this build's .debs, archive superseded older-version arxos .debs into prev/.
    let _ = step("[6/6] built packages + tidy output dir", workp, &[("KNAME", p.kname.as_str())], TIDY_DEBIAN);
    if let Some(dir) = &p.out {
        step(&format!("move packages -> {dir}"), workp, &[("OUT", dir.as_str()), ("KNAME", p.kname.as_str())],
            "mkdir -p \"$OUT\" && mv -f linux-*-$KNAME*.deb \"$OUT\"/ 2>/dev/null && echo \"  moved to $OUT\" || true")?;
    }
    println!("Install with:  sudo dpkg -i linux-image-*-{}.deb linux-headers-*.deb   (boot-test first)", p.kname);
    Ok(())
}

/// Find the extracted kernel source dir under <base>/src (the dir containing a `.config`). Walked in
/// Rust (no shell), so a base path with spaces or shell metacharacters is handled safely.
fn find_kdir(base: &Path) -> Option<String> {
    let src = base.join("src");
    // depth 1: src/<kdir>/.config ; depth 2: src/<kdir>/<sub>/.config
    let rd = std::fs::read_dir(&src).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
        if p.join(".config").is_file() { return Some(p.display().to_string()); }
        if let Ok(rd2) = std::fs::read_dir(&p) {
            for e2 in rd2.flatten() {
                let p2 = e2.path();
                if e2.file_type().map(|t| t.is_dir()).unwrap_or(false) && p2.join(".config").is_file() {
                    return Some(p2.display().to_string());
                }
            }
        }
    }
    None
}

fn first_dir(base: &Path, prefix: &str) -> Option<String> {
    let rd = std::fs::read_dir(base).ok()?;
    for e in rd.flatten() {
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if e.file_name().to_string_lossy().starts_with(prefix) {
                return Some(e.path().display().to_string());
            }
        }
    }
    None
}

fn list_patches(dir: &Path) -> Vec<String> {
    let mut v = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "patch" || x == "diff").unwrap_or(false) { v.push(p.display().to_string()); }
        }
    }
    v.sort();
    v
}

/// The patch directories bundled beside the binary for a given host. arxos applies everywhere; kali
/// (wireless injection etc.) applies on the Debian path only, matching the documented build.
pub fn bundled_patch_dirs(distro: Distro) -> Vec<std::path::PathBuf> {
    let dd = data_dir();
    match distro {
        Distro::Debian => vec![dd.join("patches/arxos"), dd.join("patches/kali")],
        _ => vec![dd.join("patches/arxos")],
    }
}

/// Expand patch sources (bundled dirs first, then user --patch args) into an ordered file list plus
/// the set of directories that carry patches (used to run their SHA256SUMS integrity check). A
/// --patch value may be a directory (its *.patch/*.diff, sorted) or a single explicit file.
pub fn collect_patches(bundled: &[std::path::PathBuf], extra: &[String]) -> (Vec<String>, Vec<String>) {
    let mut files = vec![];
    let mut dirs = vec![];
    for d in bundled {
        let fs = list_patches(d);
        if !fs.is_empty() { dirs.push(d.display().to_string()); files.extend(fs); }
    }
    for e in extra {
        let p = Path::new(e);
        if p.is_dir() {
            let fs = list_patches(p);
            if !fs.is_empty() { dirs.push(e.clone()); files.extend(fs); }
        } else {
            // an explicit file (or a nonexistent path we surface as an error at apply time)
            files.push(e.clone());
        }
    }
    (files, dirs)
}

/// Single-quote a string for safe embedding in a bash script.
fn shq(s: &str) -> String { format!("'{}'", s.replace('\'', "'\\''")) }

/// If a patch directory ships a SHA256SUMS manifest, verify it before any patch is applied. An
/// integrity failure always aborts, regardless of strict/optional (supply-chain, non-negotiable).
fn integrity_script(dirs: &[String]) -> String {
    let mut s = String::from("set -u\n");
    for d in dirs {
        s.push_str(&format!(
            "if [ -f {d}/SHA256SUMS ]; then echo \"  verifying integrity: {d}/SHA256SUMS\"; \
             ( cd {d} && sha256sum -c --strict SHA256SUMS ) || {{ echo '  PATCH INTEGRITY CHECK FAILED' >&2; exit 5; }}; fi\n",
            d = shq(d)));
    }
    s
}

/// Apply each patch with -p auto-detect (p1 then p0), skipping already-applied patches. In strict
/// mode a patch that does not apply aborts (exit 3); in optional mode it is warned and skipped.
fn apply_patches_script(files: &[String], strict: bool) -> String {
    let mut s = String::from(r#"set -u
_apply_one() {
  f="$1"
  if [ ! -f "$f" ]; then echo "  MISSING PATCH FILE: $f" >&2; return 3; fi
  for lvl in 1 0; do
    if patch -R -p$lvl --dry-run --force < "$f" >/dev/null 2>&1; then
      echo "  already applied (p$lvl), skipping: $(basename "$f")"; return 0; fi
  done
  for lvl in 1 0; do
    if patch -p$lvl --forward --dry-run < "$f" >/dev/null 2>&1; then
      patch -p$lvl --forward < "$f" >/dev/null && { echo "  applied (p$lvl): $(basename "$f")"; return 0; }
    fi
  done
  echo "  DID NOT APPLY: $(basename "$f")" >&2; return 3
}
"#);
    for f in files {
        let onfail = if strict {
            "echo \"  strict mode: aborting (patch did not apply)\" >&2; exit $rc;"
        } else {
            "echo \"  (optional: skipped)\" >&2;"
        };
        s.push_str(&format!("_apply_one {q} || {{ rc=$?; {onfail} }}\n", q = shq(f)));
    }
    s.push_str("exit 0\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn srcinfo_checksum_lookup_tracks_current_metadata() {
        let checksum = "a".repeat(128);
        let srcinfo = format!(
            "pkgbase = linux-cachyos-bore\npkgver = 7.9.1\npkgrel = 3\nsource = https://example.invalid/sched/0001-bore.patch\nb2sums = {checksum}\n"
        );
        assert_eq!(
            checksum_in_srcinfo(&srcinfo, "7.9.1", "3", "https://example.invalid/sched/0001-bore.patch"),
            Some(checksum)
        );
        assert_eq!(checksum_in_srcinfo(&srcinfo, "7.9.0", "3", "https://example.invalid/sched/0001-bore.patch"), None);
    }

    #[test]
    fn active_sources_get_ordered_checksums_from_versioned_srcinfo() {
        let root = std::env::temp_dir().join(format!("akt-srcinfo-{}", std::process::id()));
        let variant = root.join("linux-cachyos-bore");
        std::fs::create_dir_all(&variant).unwrap();
        let patch_sum = "b".repeat(128);
        let archive_sum = "c".repeat(128);
        std::fs::write(variant.join(".SRCINFO"), format!(
            "pkgbase = linux-cachyos-bore\npkgver = 7.9.1\npkgrel = 3\nsource = https://example.invalid/sched/0001-bore.patch\nb2sums = {patch_sum}\nsource = config\nb2sums = {}\n",
            "e".repeat(128)
        )).unwrap();
        let base = root.join("linux-cachyos");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join(".SRCINFO"), format!(
            "pkgbase = linux-cachyos\npkgver = 7.9.1\npkgrel = 3\nsource = https://example.invalid/kernel.tar.gz\nb2sums = {archive_sum}\nsource = config\nb2sums = {}\n",
            "d".repeat(128)
        )).unwrap();
        assert_eq!(srcinfo_checksum(&root, &base.join(".SRCINFO"), "7.9.1", "3", "https://example.invalid/sched/0001-bore.patch").unwrap(), patch_sum);
        assert_eq!(srcinfo_checksum(&root, &base.join(".SRCINFO"), "7.9.1", "3", "https://example.invalid/kernel.tar.gz").unwrap(), archive_sum);
        assert_eq!(srcinfo_checksum(&root, &base.join(".SRCINFO"), "7.9.1", "3", "config").unwrap(), "d".repeat(128));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn b2sums_array_replacement_preserves_source_verification() {
        let original = "source=('one' 'two')\nb2sums=('old1' 'old2')\n";
        let expected = vec!["SKIP".to_string(), "d".repeat(128)];
        let updated = replace_b2sums_array(original, &expected).unwrap();
        assert!(updated.contains("b2sums=(\n    'SKIP'\n"));
        assert!(updated.contains(&format!("    '{}'\n)", "d".repeat(128))));
        assert_eq!(replace_b2sums_array(&updated, &expected).unwrap(), updated);
    }

    #[test]
    fn b2sums_sync_rejects_unpaired_variant_metadata() {
        let srcinfo = "pkgbase = bad\npkgver = 7.1\npkgrel = 1\nsource = one\nsource = two\nb2sums = SKIP\n";
        assert_eq!(checksum_in_srcinfo(srcinfo, "7.1", "1", "one"), None);
    }

    // vercmp must use real version ordering, not byte ordering (the classic 7.9 vs 7.10 trap).
    #[test]
    fn vercmp_uses_version_not_byte_order() {
        assert_eq!(vercmp("7.9", "7.10").unwrap(), Ordering::Less);   // byte compare would say Greater
        assert_eq!(vercmp("7.10", "7.9").unwrap(), Ordering::Greater);
    }

    #[test]
    fn vercmp_orders_pkgrel_and_base() {
        assert_eq!(vercmp("7.2.0-2", "7.2.0-1").unwrap(), Ordering::Greater);
        assert_eq!(vercmp("7.2.5-1", "7.2.0-2").unwrap(), Ordering::Greater);
        assert_eq!(vercmp("7.2.5-1", "7.2.5-1").unwrap(), Ordering::Equal);
    }

    // A rollback target must be OLDER than what is "installed" (modeled here with two versions):
    // the guard is `vercmp(target, installed) == Less`.
    #[test]
    fn downgrade_guard_semantics() {
        // installed 7.2.5-1, archived 7.2.0-2 -> older -> allowed
        assert_eq!(vercmp("7.2.0-2", "7.2.5-1").unwrap(), Ordering::Less);
        // archived 7.2.5-1 vs installed 7.2.0-2 -> newer -> the guard would refuse
        assert_ne!(vercmp("7.2.5-1", "7.2.0-2").unwrap(), Ordering::Less);
    }

    #[test]
    fn fallback_vercmp_handles_epoch_and_release() {
        assert_eq!(fallback_vercmp("2:1.0-1", "1:9.0-99"), Ordering::Greater);
        assert_eq!(fallback_vercmp("1.2.0-2", "1.2.0-1"), Ordering::Greater);
        assert_eq!(fallback_vercmp("1.2.0", "1.2.0"), Ordering::Equal);
    }

    #[test]
    fn installed_pkgver_absent_is_none() {
        assert!(installed_pkgver("definitely-not-a-real-package-xyzzy").is_none());
    }

    #[test]
    fn build_lock_inheritance_rejects_invalid_fd() {
        // A real failing syscall, with no invalid File ownership or descriptor-close race.
        assert_eq!(inherit_build_lock(-1).unwrap_err().raw_os_error(), Some(9)); // EBADF
    }

    #[test]
    fn build_lock_inheritance_clears_only_cloexec() {
        use std::os::unix::io::AsRawFd;
        let f = std::fs::File::open("/dev/null").unwrap();
        let fd = f.as_raw_fd();
        let before = unsafe { fcntl(fd, 1, 0) }; // F_GETFD
        assert!(before >= 0);
        assert_ne!(before & 1, 0, "Rust File starts close-on-exec");
        inherit_build_lock(fd).unwrap();
        assert_eq!(unsafe { fcntl(fd, 1, 0) }, before & !1);
    }

    #[test]
    fn build_lock_is_exclusive() {
        let dir = std::env::temp_dir().join(format!("akt-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let held = acquire_build_lock(&dir).expect("first lock");
        assert!(acquire_build_lock(&dir).is_err(), "second concurrent lock must fail");
        drop(held);
        assert!(acquire_build_lock(&dir).is_ok(), "lock frees after the guard drops");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Finding 3: a build subprocess that outlives the worker keeps the lock held. We acquire the lock,
    // spawn a `sleep` that inherits the CLOEXEC-cleared lock fd, drop our own guard, and assert the lock
    // is still held (blocked) until the surviving child exits, then frees.
    #[test]
    fn build_lock_survives_via_inherited_child_fd() {
        let dir = std::env::temp_dir().join(format!("akt-lock-child-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let held = acquire_build_lock(&dir).expect("first lock");
        let mut child = std::process::Command::new("sleep").arg("30")
            .stdin(std::process::Stdio::null())
            .spawn().expect("spawn holder");
        std::thread::sleep(std::time::Duration::from_millis(250)); // let it exec (inheriting the fd)
        drop(held);                                                // our own fd is gone now
        let blocked = acquire_build_lock(&dir).is_err();
        let _ = child.kill(); let _ = child.wait();
        std::thread::sleep(std::time::Duration::from_millis(250)); // let the OFD close
        let free_after = acquire_build_lock(&dir).is_ok();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(blocked, "lock must stay held by the surviving child after the parent guard drops");
        assert!(free_after, "lock must free once the surviving child exits");
    }
}
