// The build engine. Orchestrates the documented linux-arxos flow for each host. Every external step
// is a shell command run with inherited stdio (live output). The config stage HARD-VERIFIES the
// ARXOS delta before the multi-hour compile, so a stripped option aborts here, not after hours.
use crate::{data_dir, gate, resources, Distro, Plan};
use std::io;
use std::path::Path;
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

pub fn run(plan: Plan, dry: bool) -> io::Result<()> {
    if plan.distro == Distro::Unknown {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "unsupported host (need Arch- or Debian-family)"));
    }
    print_plan(&plan);
    if dry {
        let work = std::env::var("AKT_WORK").unwrap_or_else(|_| format!("{}/build/{}-{}", data_dir().display(), plan.distro.name(), plan.flavor.key));
        resources::detect(Path::new(&work))?.report();
        println!("[dry-run] no build performed. Re-run without --dry-run to build.");
        return Ok(());
    }
    match plan.distro {
        Distro::Arch => build_arch(&plan),
        Distro::Debian => build_debian(&plan),
        Distro::Unknown => unreachable!(),
    }
}

fn build_arch(p: &Plan) -> io::Result<()> {
    let dd = data_dir();
    let work = std::env::var("AKT_WORK").unwrap_or_else(|_| format!("{}/build/arch-{}", dd.display(), p.flavor.key));
    std::fs::create_dir_all(&work)?;
    let workp = Path::new(&work);
    let frag = dd.join("config/arxos.fragment");
    let initial = resources::detect(workp)?;
    initial.report();
    let jobs = initial.jobs.to_string();

    // 1. fetch the CachyOS recipe (rolling latest).
    step("[1/5] fetch CachyOS linux-cachyos", workp, &[],
        "if [ ! -d cos/.git ]; then git clone -q https://github.com/CachyOS/linux-cachyos.git cos; \
         else git -C cos pull -q || true; fi")?;

    let base = format!("{work}/cos/linux-cachyos");
    let basep = Path::new(&base);
    let env = [
        ("_HZ_ticks", "1000"), ("_tcp_bbr3", "yes"), ("_processor_opt", "GENERIC_V3"),
        ("_cpusched", p.flavor.sched), ("_use_llvm_lto", p.flavor.lto), ("_preempt", "full"),
        ("_build_zfs", "no"), ("_build_nvidia_open", "no"), ("_cachy_config", "yes"),
        ("PACKAGER", "ARXOS <team@arxos.uk>"), ("ARXOS_PKGREL", p.pkgrel.as_str()), ("KNAME", p.kname.as_str()),
        ("ARXOS_BUILD_JOBS", jobs.as_str()),
    ];
    resources::limit_recipe(&basep.join("PKGBUILD"))?;

    // 2. de-brand + pkgrel (matches linux-arxos build.sh).
    step(&format!("[2/5] de-brand recipe -> linux-{}", p.kname), basep, &env,
        "sed -i \"s/_pkgsuffix=cachyos/_pkgsuffix=$KNAME/g; s/KBUILD_BUILD_HOST=cachyos/KBUILD_BUILD_HOST=$KNAME/\" PKGBUILD; \
         sed -i \"s/^pkgrel=.*/pkgrel=${ARXOS_PKGREL:-1}/\" PKGBUILD")?;

    // 3. extract + prepare.
    step("[3/5] extract + prepare", basep, &env, "rm -rf src pkg; makepkg -o --skipinteg --nodeps --noconfirm")?;

    // 4. overlay ARXOS config + patches, RE-ASSERT + HARD-VERIFY the delta (fail-fast).
    let kdir = find_kdir(basep).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "kernel source dir not found under src/"))?;
    let (pfiles, pdirs) = collect_patches(&bundled_patch_dirs(p.distro), &p.extra_patches);
    if !pfiles.is_empty() {
        if !pdirs.is_empty() { step("verify patch integrity", Path::new(&kdir), &[], &integrity_script(&pdirs))?; }
        step(&format!("apply {} patch(es) [{}]", pfiles.len(), if p.patch_strict { "strict" } else { "optional" }),
            Path::new(&kdir), &[], &apply_patches_script(&pfiles, p.patch_strict))?;
    }
    step("[4/5] overlay ARXOS config + verify hardening delta", Path::new(&kdir), &env,
        &format!(
        "./scripts/kconfig/merge_config.sh -m .config {frag}; ./scripts/config -d RUST; make LLVM=1 olddefconfig; \
         ./scripts/config --enable INIT_ON_FREE_DEFAULT_ON --enable INIT_ON_ALLOC_DEFAULT_ON --disable HIBERNATION \
           --enable INTEL_IOMMU_DEFAULT_ON --enable IOMMU_DEFAULT_DMA_STRICT --enable SECURITY_LOCKDOWN_LSM_EARLY \
           --enable INTEGRITY --enable IMA --enable IMA_DEFAULT_HASH_SHA256 --enable SECURITY_DMESG_RESTRICT \
           --enable LIVEPATCH; make LLVM=1 olddefconfig",
        frag = frag.display()))?;
    if !gate::verify(&format!("{kdir}/.config"), false) {
        return Err(io::Error::new(io::ErrorKind::Other, "ARXOS hardening delta missing after config — aborting before compile"));
    }

    // 5. build (-f overwrites a stale same-version pkg from a prior, e.g. wrong-config, build).
    let current = resources::detect(workp)?;
    current.report();
    resources::limit_kernel(Path::new(&kdir), current.link_jobs)?;
    let jobs = current.jobs.to_string();
    let mut compile_env = env.to_vec();
    compile_env.push(("ARXOS_BUILD_JOBS", jobs.as_str()));
    step("[5/5] compile (ThinLTO — long)", basep, &compile_env, "makepkg -e -f --skipinteg --nodeps --noconfirm")?;
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

/// Roll back to the previously-built kernel: reinstall the newest linux-<kname> package archived in
/// prev/. This is the mechanism the ArxOS kernel gate proved is REQUIRED — a same-name pacman upgrade
/// deletes the prior version's /usr/lib/modules, so a saved kernel image alone cannot boot cleanly;
/// reinstalling the retained package restores the module tree + rebuilds the initramfs via pacman hooks.
pub fn rollback(p: &Plan) -> io::Result<()> {
    if p.distro != Distro::Arch {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "akt rollback currently supports the Arch path only"));
    }
    let dd = data_dir();
    let work = std::env::var("AKT_WORK").unwrap_or_else(|_| format!("{}/build/arch-{}", dd.display(), p.flavor.key));
    let prev = format!("{work}/cos/linux-cachyos/prev");
    // Use `find` (empty on no-match), never `ls` on a glob: an unmatched glob makes `ls` list the
    // cwd instead. Filter to the wanted class BEFORE picking the newest by mtime, so the kernel
    // package and its headers never shadow each other.
    let script = format!(
        "set -e; \
         newest() {{ find {prev} -maxdepth 1 -type f -name 'linux-{k}-*.pkg.tar.*' -printf '%T@ %p\\n' 2>/dev/null \
                    | grep -vE 'debug' | grep -E \"$1\" | sort -rn | head -1 | cut -d' ' -f2-; }}; \
         PKG=$(newest '/linux-{k}-[0-9]'); \
         HDR=$(newest '/linux-{k}-headers-'); \
         if [ -z \"$PKG\" ]; then echo 'no archived previous package in {prev} (nothing to roll back to)'; exit 1; fi; \
         echo \"rolling back to: $(basename \"$PKG\")${{HDR:+ + $(basename \"$HDR\")}}\"; \
         sudo pacman -U --noconfirm \"$PKG\" ${{HDR:+\"$HDR\"}}; \
         echo 'installed. Reboot and select this kernel; boot-test in a throwaway first.'",
        prev = prev, k = p.kname);
    step(&format!("rollback linux-{} from prev/", p.kname), Path::new(&dd), &[], &script)
}

fn build_debian(p: &Plan) -> io::Result<()> {
    let dd = data_dir();
    let work = std::env::var("AKT_WORK").unwrap_or_else(|_| format!("{}/build/debian-{}", dd.display(), p.flavor.key));
    std::fs::create_dir_all(&work)?;
    let workp = Path::new(&work);
    let frag = dd.join("config/arxos.fragment");
    resources::detect(workp)?.report();

    // 1. Debian kernel source. `linux-source` (from the distro) is the reproducible route; the git
    //    tree is the alternative. We take the distro source so the .deb tooling matches the host.
    step("[1/6] fetch Debian kernel source", workp, &[],
        "if ! ls linux-source-*/ >/dev/null 2>&1; then \
           sudo apt-get -y install linux-source build-essential fakeroot libncurses-dev bison flex libssl-dev libelf-dev; \
           tar xaf /usr/src/linux-source-*.tar.* ; fi")?;
    // find the extracted source dir
    let src = first_dir(workp, "linux-source-").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound,
        "linux-source-*/ not found — install the `linux-source` package for your kernel"))?;
    let srcp = Path::new(&src);

    // 2. start from the running kernel's config, then overlay CachyOS-style tunes + the ARXOS delta.
    step("[2/6] seed config from the running kernel", srcp, &[],
        "cp -f /boot/config-$(uname -r) .config 2>/dev/null || make defconfig")?;
    step("[3/6] apply CachyOS-style performance tunes", srcp, &[],
        "./scripts/config --enable SCHED_BORE 2>/dev/null; \
         ./scripts/config --set-val HZ 1000 --enable HZ_1000 --enable PREEMPT --enable TCP_CONG_BBR --set-str DEFAULT_TCP_CONG bbr \
           --enable TRANSPARENT_HUGEPAGE --enable LRU_GEN --enable LRU_GEN_ENABLED; true")?;

    // 3. overlay the ARXOS hardening delta + Kali/ARXOS patches, then verify.
    let (pfiles, pdirs) = collect_patches(&bundled_patch_dirs(p.distro), &p.extra_patches);
    if !pfiles.is_empty() {
        if !pdirs.is_empty() { step("verify patch integrity", srcp, &[], &integrity_script(&pdirs))?; }
        step(&format!("apply {} patch(es) [{}]", pfiles.len(), if p.patch_strict { "strict" } else { "optional" }),
            srcp, &[], &apply_patches_script(&pfiles, p.patch_strict))?;
    }
    step("[4/6] overlay ARXOS config + verify hardening delta", srcp, &[],
        &format!(
        "./scripts/kconfig/merge_config.sh -m .config {frag}; make olddefconfig; \
         ./scripts/config --enable INIT_ON_FREE_DEFAULT_ON --enable INIT_ON_ALLOC_DEFAULT_ON --disable HIBERNATION \
           --enable INTEL_IOMMU_DEFAULT_ON --enable IOMMU_DEFAULT_DMA_STRICT --enable SECURITY_LOCKDOWN_LSM_EARLY \
           --enable INTEGRITY --enable IMA --enable IMA_DEFAULT_HASH_SHA256 --enable SECURITY_DMESG_RESTRICT \
           --enable LIVEPATCH; make olddefconfig",
        frag = frag.display()))?;
    if !gate::verify(&format!("{src}/.config"), false) {
        return Err(io::Error::new(io::ErrorKind::Other, "ARXOS hardening delta missing after config — aborting before compile"));
    }

    // 4. build .deb packages.
    let current = resources::detect(workp)?;
    current.report();
    resources::limit_kernel(srcp, current.link_jobs)?;
    let jobs = current.jobs.to_string();
    step("[5/6] build .deb (long)", srcp, &[("KNAME", p.kname.as_str()), ("ARXOS_BUILD_JOBS", jobs.as_str())],
        "make -j\"${ARXOS_BUILD_JOBS:?}\" LOCALVERSION=-$KNAME bindeb-pkg")?;
    // tidy: keep this build's .debs, archive superseded older-version arxos .debs into prev/.
    let _ = step("[6/6] built packages + tidy output dir", workp, &[("KNAME", p.kname.as_str())], TIDY_DEBIAN);
    if let Some(dir) = &p.out {
        step(&format!("move packages -> {dir}"), workp, &[("OUT", dir.as_str()), ("KNAME", p.kname.as_str())],
            "mkdir -p \"$OUT\" && mv -f linux-*-$KNAME*.deb \"$OUT\"/ 2>/dev/null && echo \"  moved to $OUT\" || true")?;
    }
    println!("Install with:  sudo dpkg -i linux-image-*-{}.deb linux-headers-*.deb   (boot-test first)", p.kname);
    Ok(())
}

fn find_kdir(base: &Path) -> Option<String> {
    let out = Command::new("bash").arg("-c")
        .arg(format!("find '{}/src' -maxdepth 2 -name .config -printf '%h\\n' 2>/dev/null | head -1", base.display()))
        .output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
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
