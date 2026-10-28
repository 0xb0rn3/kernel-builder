//! `akt install` — install a built kernel into the running system while GUARANTEEING that the
//! kernel you are running right now survives the operation.
//!
//! WHY THIS EXISTS. A same-name `pacman -U` upgrade removes the previous version's
//! /usr/lib/modules tree. Keeping the old vmlinuz and initramfs is not enough: without its modules
//! that kernel boots into a system with no storage, no network and no input drivers. akt already
//! warned about this and left the install to the user, and `akt rollback` can only reach packages
//! akt itself archived into prev/.
//!
//! That leaves a real hole, measured on a real machine: the running kernel was linux-arxos 7.2.9-1
//! with ZERO copies in the pacman cache and ZERO in any prev/ directory. Installing a same-name
//! build would have deleted the only existing copy of the running kernel, and there would have been
//! nothing anywhere to restore it from.
//!
//! So before anything is installed, this makes the running kernel restorable: it finds the package
//! that owns the running module tree and, if no package file for that exact version exists anywhere,
//! REBUILDS one from the files the package manager records as installed. After the install it checks
//! the running module tree is still present and puts it back from that saved package if it is not.
//!
//! PER DISTRO. akt builds for Arch and Debian today and is meant to grow, so installation dispatches
//! on the target rather than assuming one package manager. Arch is the dangerous case: kernel
//! packages share one name, so a same-version install replaces the module tree. Debian names each
//! kernel package after its version, so a new kernel installs ALONGSIDE the running one and the
//! hazard largely does not arise; it is still verified rather than assumed. Adding Fedora (dnf,
//! kernel-core, and its own multi-kernel retention) means one more arm of this dispatch and a
//! matching restorable-copy step, nothing else.

use crate::build::{pkg_identity, vercmp};
use crate::Plan;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

fn running_release() -> io::Result<String> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/osrelease")?.trim().to_string())
}

fn sh(cmd: &str) -> io::Result<String> {
    let o = Command::new("sh").arg("-c").arg(cmd).output()?;
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn is_pkg(n: &str) -> bool {
    n.ends_with(".pkg.tar.zst") || n.ends_with(".pkg.tar.xz")
        || n.ends_with(".pkg.tar.gz") || n.ends_with(".pkg.tar")
}

/// Highest-version package named `want` in `dir`, paired by identity read from .PKGINFO.
fn newest_in(dir: &Path, want: &str) -> io::Result<Option<(String, PathBuf)>> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(None) };
    let mut best: Option<(String, PathBuf)> = None;
    for e in rd.flatten() {
        let path = e.path();
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) { continue; }
        if !is_pkg(&e.file_name().to_string_lossy()) { continue; }
        let Some((name, ver)) = pkg_identity(&path) else { continue };
        if name != want { continue; }
        best = match best {
            None => Some((ver, path)),
            Some(b) => {
                if vercmp(&ver, &b.0)? == std::cmp::Ordering::Greater { Some((ver, path)) } else { Some(b) }
            }
        };
    }
    Ok(best)
}

/// Find an existing package file for `pkg`-`ver`, in the pacman cache or any directory given.
fn find_package_file(pkg: &str, ver: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    let mut dirs = vec![PathBuf::from("/var/cache/pacman/pkg")];
    dirs.extend_from_slice(extra);
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if !is_pkg(&e.file_name().to_string_lossy()) { continue; }
                if let Some((n, v)) = pkg_identity(&p) {
                    if n == pkg && v == ver { return Some(p); }
                }
            }
        }
    }
    None
}

/// Rebuild a package from what pacman records as installed. This is the part that closes the hole:
/// a kernel that was installed but whose package file no longer exists anywhere can still be made
/// restorable, because the local database lists every file it owns and ships an mtree to check them
/// against. Runs as root because it reads files the user cannot.
fn repack_installed(pkg: &str, ver: &str, out_dir: &Path) -> io::Result<PathBuf> {
    let db = format!("/var/lib/pacman/local/{pkg}-{ver}");
    if !Path::new(&db).join("files").exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound,
            format!("pacman has no file list for {pkg} {ver} at {db}; cannot rebuild a package from it")));
    }
    std::fs::create_dir_all(out_dir)?;
    let out = out_dir.join(format!("{pkg}-{ver}-x86_64.pkg.tar.zst"));
    println!("  rebuilding a restorable package for the running kernel from pacman's own file list");
    let script = r#"
set -eu
pkg="$1"; ver="$2"; out="$3"
db="/var/lib/pacman/local/${pkg}-${ver}"
stage="$(mktemp -d)"; trap 'rm -rf "$stage"' EXIT
list="$(mktemp)"; awk '!/^%/ && NF' "$db/files" > "$list"
missing=0
while read -r f; do [ -e "/$f" ] || missing=$((missing+1)); done < "$list"
[ "$missing" -eq 0 ] || { echo "  $missing recorded file(s) are missing from the filesystem; refusing to build an incomplete package" >&2; exit 1; }
rsync -aHAX --numeric-ids --files-from="$list" / "$stage/"
python3 - "$db/desc" "$stage/.PKGINFO" <<'PY'
import sys
src,dst=sys.argv[1],sys.argv[2]
b,k={},None
for line in open(src):
    line=line.rstrip("\n")
    if line.startswith("%") and line.endswith("%"): k=line.strip("%"); b[k]=[]
    elif line and k: b[k].append(line)
m={"NAME":"pkgname","VERSION":"pkgver","DESC":"pkgdesc","URL":"url","ARCH":"arch","BUILDDATE":"builddate",
   "PACKAGER":"packager","SIZE":"size","LICENSE":"license","REPLACES":"replaces","GROUPS":"group",
   "CONFLICTS":"conflict","PROVIDES":"provides","DEPENDS":"depend","OPTDEPENDS":"optdepend","MAKEDEPENDS":"makedepend"}
out=[]
for key in ("NAME","VERSION","DESC","URL","BUILDDATE","PACKAGER","SIZE","ARCH","LICENSE","REPLACES",
            "GROUPS","CONFLICTS","PROVIDES","DEPENDS","OPTDEPENDS","MAKEDEPENDS"):
    for v in b.get(key,[]): out.append(f"{m[key]} = {v}")
open(dst,"w").write("\n".join(out)+"\n")
PY
[ -f "$db/mtree" ] && cp "$db/mtree" "$stage/.MTREE"
( cd "$stage" && tar --zstd -cf "$out" .PKGINFO $( [ -f .MTREE ] && echo .MTREE ) $(ls -A | grep -vE '^\.(PKGINFO|MTREE)$') )
rm -f "$list"
"#;
    let status = Command::new("sudo").arg("bash").arg("-s").arg("--")
        .arg(pkg).arg(ver).arg(&out)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.as_mut().unwrap().write_all(script.as_bytes())?;
            c.wait()
        })?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other,
            format!("could not rebuild a restorable package for {pkg} {ver}")));
    }
    // Trust nothing: pacman must agree the result is that package at that version.
    match pkg_identity(&out) {
        Some((n, v)) if n == pkg && v == ver => {
            println!("  saved {} ({})", out.display(), pkg_size(&out));
            Ok(out)
        }
        other => Err(io::Error::new(io::ErrorKind::Other,
            format!("rebuilt package does not identify as {pkg} {ver} (got {other:?})"))),
    }
}

fn pkg_size(p: &Path) -> String {
    std::fs::metadata(p).map(|m| format!("{} MiB", m.len() / 1024 / 1024)).unwrap_or_else(|_| "?".into())
}

fn pacman_u(paths: &[PathBuf]) -> io::Result<()> {
    let mut args: Vec<std::ffi::OsString> =
        vec!["pacman".into(), "-U".into(), "--noconfirm".into(), "--".into()];
    for p in paths { args.push(p.clone().into()); }
    let status = Command::new("sudo").args(&args).status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "pacman -U failed"));
    }
    Ok(())
}

/// Install the newest built kernel (and matching headers) into the running system.
fn install_arch(p: &Plan, keep_running: bool) -> io::Result<()> {
    let work = crate::build::work_dir(crate::Distro::Arch, &p.work_key())?;
    let outdir = p.out.clone().unwrap_or_else(|| work.join("cos/linux-cachyos").display().to_string());
    let outdir = PathBuf::from(outdir);
    let kpkg = format!("linux-{}", p.kname);
    let hpkg = format!("linux-{}-headers", p.kname);

    let Some((kver, kpath)) = newest_in(&outdir, &kpkg)? else {
        return Err(io::Error::new(io::ErrorKind::NotFound,
            format!("no {kpkg} package in {} - build one first", crate::display_path(&outdir.display().to_string()))));
    };
    println!("installing {} {}", kpkg, kver);
    println!("  kernel : {}", kpath.display());
    let hpath = newest_in(&outdir, &hpkg)?.filter(|(hv, _)| *hv == kver).map(|(_, p)| p);
    match &hpath {
        Some(h) => println!("  headers: {}", h.display()),
        None => println!("  headers: none matching {kver} (installing the kernel only)"),
    }

    // ---- make the running kernel restorable BEFORE touching anything ----------------------
    let running = running_release()?;
    let moddir = PathBuf::from(format!("/usr/lib/modules/{running}"));
    let mut saved: Option<PathBuf> = None;
    if keep_running {
        println!("keeping the running kernel available: {running}");
        if !moddir.exists() {
            println!("  note: {} does not exist, so there is no running module tree to protect", moddir.display());
        } else {
            let owner = sh(&format!("pacman -Qoq {} 2>/dev/null | head -1", moddir.display()))?;
            if owner.is_empty() {
                println!("  note: no package owns {}; it cannot be restored by reinstalling a package.", moddir.display());
                println!("        Nothing will remove it either, unless a package claims that path.");
            } else {
                let iver = sh(&format!("pacman -Q {owner} 2>/dev/null | awk '{{print $2}}'"))?;
                println!("  running kernel comes from {owner} {iver}");
                let prev = outdir.join("prev");
                match find_package_file(&owner, &iver, &[prev.clone(), outdir.clone()]) {
                    Some(f) => { println!("  already restorable from {}", f.display()); saved = Some(f); }
                    None => {
                        println!("  no package file for {owner} {iver} exists in the pacman cache or prev/");
                        saved = Some(repack_installed(&owner, &iver, &prev)?);
                    }
                }
            }
        }
        // A different version installs alongside; only a same-version install replaces the tree.
        if kver == sh("pacman -Q linux-arxos 2>/dev/null | awk '{print $2}'").unwrap_or_default() {
            println!("  this build has the SAME version as what is installed, so it will replace the");
            println!("  running module tree. That is what the saved package above is for.");
        }
    } else {
        println!("WARNING: --no-keep-running was given. A same-name install removes the running");
        println!("         kernel's modules and nothing here will put them back.");
    }

    // ---- install -------------------------------------------------------------------------
    let mut to_install = vec![kpath.clone()];
    if let Some(h) = &hpath { to_install.push(h.clone()); }
    pacman_u(&to_install)?;

    // ---- verify the running kernel survived, and restore it if not -----------------------
    if keep_running && !moddir.exists() {
        match &saved {
            Some(f) => {
                println!("the running kernel's modules were removed by the install; restoring from {}", f.display());
                pacman_u(std::slice::from_ref(f))?;
                if moddir.exists() {
                    println!("restored {}", moddir.display());
                } else {
                    return Err(io::Error::new(io::ErrorKind::Other, format!(
                        "restore ran but {} is still missing — do NOT reboot; investigate before restarting",
                        moddir.display())));
                }
            }
            None => return Err(io::Error::new(io::ErrorKind::Other, format!(
                "{} was removed and no saved package exists to restore it — do NOT reboot", moddir.display()))),
        }
    }

    println!();
    println!("installed {kpkg} {kver}.");
    if keep_running && moddir.exists() {
        println!("the kernel you are running ({running}) is still installed and bootable.");
    }
    println!("boot entries are regenerated by pacman's hooks; boot-test in a throwaway VM before");
    println!("relying on the new kernel, and keep the running one selectable in your bootloader.");
    Ok(())
}

/// Debian. Kernel packages here are named per version (linux-image-<release>), so installing a new
/// kernel does not remove the running one: both stay installed and both get boot entries. That is a
/// property of the packaging, not a guarantee from us, so it is verified after the install rather
/// than assumed, and the running image package is reported so the user knows what to keep.
fn install_debian(p: &Plan, keep_running: bool) -> io::Result<()> {
    let work = crate::build::work_dir(crate::Distro::Debian, &p.work_key())?;
    let outdir = PathBuf::from(p.out.clone().unwrap_or_else(|| work.display().to_string()));
    let debs = sh(&format!(
        "ls -1 {}/linux-image-*{}*.deb {}/linux-headers-*.deb 2>/dev/null",
        outdir.display(), p.kname, outdir.display()))?;
    let debs: Vec<PathBuf> = debs.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect();
    if debs.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound,
            format!("no linux-image .deb for {} in {} - build one first", p.kname, crate::display_path(&outdir.display().to_string()))));
    }
    for d in &debs { println!("  package: {}", d.display()); }

    let running = running_release()?;
    let moddir = PathBuf::from(format!("/lib/modules/{running}"));
    if keep_running {
        println!("keeping the running kernel available: {running}");
        let owner = sh(&format!("dpkg -S {} 2>/dev/null | head -1 | cut -d: -f1", moddir.display()))?;
        if owner.is_empty() {
            println!("  note: no package owns {}", moddir.display());
        } else {
            println!("  running kernel comes from {owner}");
            println!("  Debian keeps kernels under per-version package names, so installing this build");
            println!("  does not remove {owner}. Leave it installed and it stays bootable.");
        }
    }

    let mut args: Vec<std::ffi::OsString> = vec!["dpkg".into(), "-i".into(), "--".into()];
    for d in &debs { args.push(d.clone().into()); }
    let status = Command::new("sudo").args(&args).status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "dpkg -i failed"));
    }
    if keep_running && !moddir.exists() {
        return Err(io::Error::new(io::ErrorKind::Other, format!(
            "{} disappeared during the install, which is not expected on Debian — do NOT reboot",
            moddir.display())));
    }
    println!();
    println!("installed. the kernel you are running ({running}) is still installed and bootable.");
    println!("update-grub runs from the package hooks; boot-test before relying on the new kernel.");
    Ok(())
}

/// Install the newest built kernel into the running system, per distro.
pub fn install_to_system(p: &Plan, keep_running: bool) -> io::Result<()> {
    match p.distro {
        crate::Distro::Arch => install_arch(p, keep_running),
        crate::Distro::Debian => install_debian(p, keep_running),
        // Deliberately explicit rather than a catch-all: when Fedora (or another distro) gains a
        // build path it needs its own install arm with its own restorable-copy step, and a silent
        // fallback here would install nothing while appearing to succeed.
        other => Err(io::Error::new(io::ErrorKind::Unsupported, format!(
            "akt install has no path for {:?} yet. Arch and Debian are implemented; Fedora and other \
             distributions are planned and each needs its own package-manager arm, because what keeps \
             the running kernel restorable differs per packaging. Build the package and install it with \
             your distribution's tool in the meantime.", other))),
    }
}
