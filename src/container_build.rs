//! Cross-distro build runner. Runs the same akt engine inside a disposable target-distro container.
//! Source and configuration are embedded so an installed binary does not require a checkout.
use crate::{Distro, Plan};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const PROJECT_FILES: &[(&str, &str)] = &[
    ("Cargo.toml", include_str!("../Cargo.toml")),
    ("Cargo.lock", include_str!("../Cargo.lock")),
    ("src/main.rs", include_str!("main.rs")),
    ("src/build.rs", include_str!("build.rs")),
    ("src/explain.rs", include_str!("explain.rs")),
    ("src/gate.rs", include_str!("gate.rs")),
    ("src/resources.rs", include_str!("resources.rs")),
    ("src/server.rs", include_str!("server.rs")),
    ("src/sysinfo.rs", include_str!("sysinfo.rs")),
    ("src/container_build.rs", include_str!("container_build.rs")),
    ("config/arxos.fragment", include_str!("../config/arxos.fragment")),
    ("config/flavor-default.fragment", include_str!("../config/flavor-default.fragment")),
    ("patches/arxos/README.md", include_str!("../patches/arxos/README.md")),
    ("patches/kali/README.md", include_str!("../patches/kali/README.md")),
    ("ui/index.html", include_str!("../ui/index.html")),
];

struct Cleanup { dir: PathBuf, runtime: String, image: String }
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new(&self.runtime).args(["rmi", "--force", &self.image])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
fn numeric_id(flag: &str) -> io::Result<u32> {
    let out = Command::new("id").arg(flag).output()?;
    if !out.status.success() { return Err(invalid("cannot determine current user ID for a container build")); }
    String::from_utf8_lossy(&out.stdout).trim().parse::<u32>()
        .map_err(|_| invalid("cannot parse current user ID"))
}
fn runtime() -> io::Result<String> {
    for name in ["podman", "docker"] {
        if Command::new(name).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status()
            .map(|s| s.success()).unwrap_or(false) {
            return Ok(name.to_string());
        }
    }
    Err(io::Error::new(io::ErrorKind::NotFound,
        "cross-distro builds need Podman or Docker; install one, then retry --target-distro arch|debian"))
}
fn temp_root() -> io::Result<PathBuf> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    for attempt in 0..10 {
        let path = std::env::temp_dir().join(format!("akt-cross-{}-{nonce}-{attempt}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "cannot allocate a temporary cross-build directory"))
}
fn write_project(root: &Path) -> io::Result<()> {
    for (relative, content) in PROJECT_FILES {
        let path = root.join(relative);
        if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
        fs::write(path, content)?;
    }
    Ok(())
}
fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        return Err(invalid(format!("refusing symlink inside external patch input: {}", source.display())));
    }
    if meta.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if meta.is_file() {
        if let Some(parent) = destination.parent() { fs::create_dir_all(parent)?; }
        fs::copy(source, destination)?;
    } else {
        return Err(invalid(format!("external patch input is not a regular file/directory: {}", source.display())));
    }
    Ok(())
}
fn absolute_dir(path: &Path) -> io::Result<PathBuf> {
    let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
    fs::create_dir_all(&abs)?;
    let canonical = fs::canonicalize(&abs)?;
    if canonical.to_string_lossy().contains(':') {
        return Err(invalid("container bind-mount paths containing ':' are not supported"));
    }
    Ok(canonical)
}
fn output_path(work: &Path, plan: &Plan) -> PathBuf {
    plan.out.as_ref().map(PathBuf::from).unwrap_or_else(|| work.to_path_buf())
}
fn containerfile(target: Distro) -> &'static str {
    match target {
        Distro::Arch => r#"FROM archlinux:latest
ARG BUILD_UID=1000
ARG BUILD_GID=1000
RUN pacman -Syu --noconfirm && pacman -S --noconfirm --needed base-devel rust cargo git clang llvm lld sudo bc cpio flex bison openssl zstd pahole && \
    groupadd -g "$BUILD_GID" aktbuild && useradd -m -u "$BUILD_UID" -g "$BUILD_GID" -s /bin/bash aktbuild && \
    printf '%s\n' 'aktbuild ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/aktbuild && chmod 0440 /etc/sudoers.d/aktbuild
COPY --chown=$BUILD_UID:$BUILD_GID . /src
USER $BUILD_UID:$BUILD_GID
WORKDIR /src
RUN cargo build --release
ENTRYPOINT ["/src/target/release/akt"]
"#,
        Distro::Debian => r#"FROM debian:stable-slim
ARG BUILD_UID=1000
ARG BUILD_GID=1000
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential cargo rustc git sudo clang llvm lld bc bison flex libssl-dev libelf-dev libncurses-dev \
    zstd xz-utils cpio kmod ca-certificates && rm -rf /var/lib/apt/lists/* && \
    groupadd -g "$BUILD_GID" aktbuild && useradd -m -u "$BUILD_UID" -g "$BUILD_GID" -s /bin/bash aktbuild && \
    printf '%s\n' 'aktbuild ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/aktbuild && chmod 0440 /etc/sudoers.d/aktbuild
COPY --chown=$BUILD_UID:$BUILD_GID . /src
USER $BUILD_UID:$BUILD_GID
WORKDIR /src
RUN cargo build --release
ENTRYPOINT ["/src/target/release/akt"]
"#,
        Distro::Unknown => "",
    }
}
fn stage_extra_patches(root: &Path, args: &[String]) -> io::Result<Vec<String>> {
    let mut staged = Vec::new();
    let mut index = 0usize;
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == "--patch" {
            let raw = args.get(i + 1).ok_or_else(|| invalid("--patch needs a file or directory"))?;
            let original = fs::canonicalize(raw).map_err(|e| io::Error::new(e.kind(), format!("cannot read --patch {}: {e}", raw)))?;
            let meta = fs::metadata(&original)?;
            let dest = root.join("external-patches").join(format!("patch-{index}"));
            if meta.is_dir() {
                copy_tree(&original, &dest)?;
                staged.push(format!("/src/external-patches/patch-{index}"));
            } else if meta.is_file() {
                let name = original.file_name().ok_or_else(|| invalid("patch file has no filename"))?;
                copy_tree(&original, &dest.join(name))?;
                staged.push(format!("/src/external-patches/patch-{index}/{}", name.to_string_lossy()));
            } else {
                return Err(invalid(format!("--patch must be a regular file or directory: {}", original.display())));
            }
            index += 1;
            i += 2;
        } else { i += 1; }
    }
    Ok(staged)
}
fn child_args(args: &[String], staged_patches: &[String]) -> io::Result<Vec<String>> {
    let mut out = vec!["build".to_string()];
    let mut i = 1usize;
    let mut patch_idx = 0usize;
    let mut out_added = false;
    while i < args.len() {
        match args[i].as_str() {
            "--target-distro" | "--out" | "--patch" => {
                if i + 1 >= args.len() { return Err(invalid(format!("{} needs a value", args[i]))); }
                match args[i].as_str() {
                    "--target-distro" => { i += 2; }
                    "--out" => {
                        if !out_added { out.extend(["--out".to_string(), "/out".to_string()]); out_added = true; }
                        i += 2;
                    }
                    "--patch" => {
                        let value = staged_patches.get(patch_idx).ok_or_else(|| invalid("internal error staging --patch"))?;
                        out.extend(["--patch".to_string(), value.clone()]);
                        patch_idx += 1;
                        i += 2;
                    }
                    _ => unreachable!(),
                }
            }
            arg => { out.push(arg.to_string()); i += 1; }
        }
    }
    if !out_added { out.extend(["--out".to_string(), "/out".to_string()]); }
    Ok(out)
}

pub fn run(plan: &Plan, args: &[String], host: Distro) -> io::Result<()> {
    if !matches!(plan.distro, Distro::Arch | Distro::Debian) {
        return Err(invalid("target distro must be arch or debian"));
    }
    if std::env::consts::ARCH != "x86_64" {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "current kernel recipes only support x86_64"));
    }
    let uid = numeric_id("-u")?;
    let gid = numeric_id("-g")?;
    if uid == 0 { return Err(invalid("do not run cross-distro builds as root; run akt as your regular user")); }
    let runtime = runtime()?;
    let work_path = crate::build::work_dir(plan.distro, plan.flavor.key)?;
    fs::create_dir_all(&work_path)?;
    let work = fs::canonicalize(&work_path)?;
    let output = absolute_dir(&output_path(&work, plan))?;
    let root = temp_root()?;
    let image = format!("akt-cross-{}-{}", plan.distro.name(), std::process::id());
    let cleanup = Cleanup { dir: root.clone(), runtime: runtime.clone(), image: image.clone() };
    write_project(&root)?;
    let staged_patches = stage_extra_patches(&root, args)?;
    fs::write(root.join("Containerfile"), containerfile(plan.distro))?;
    println!("akt: cross-distro build: host={} target={} via {}", host.name(), plan.distro.name(), runtime);
    println!("akt: dependencies are installed in a disposable container; host packages are not modified");
    let uid_arg = format!("BUILD_UID={uid}");
    let gid_arg = format!("BUILD_GID={gid}");
    let status = Command::new(&runtime)
        .args(["build", "--build-arg", &uid_arg, "--build-arg", &gid_arg, "-t", &image, "-f", "Containerfile"])
        .arg(&root).status()?;
    if !status.success() { return Err(io::Error::new(io::ErrorKind::Other, format!("{runtime} image build failed"))); }
    let child = child_args(args, &staged_patches)?;
    let work_mount = format!("{}:/work", work.display());
    let out_mount = format!("{}:/out", output.display());
    let mut run = Command::new(&runtime);
    run.args(["run", "--rm"]);
    if runtime == "podman" { run.args(["--userns=keep-id"]); }
    run.args(["-e", "AKT_WORK=/work", "-e", "XDG_CACHE_HOME=/work/cache", "-v", &work_mount, "-v", &out_mount, "-w", "/src", &image]);
    if let Ok(cap) = std::env::var("AKT_MAX_JOBS") {
        run.args(["-e", &format!("AKT_MAX_JOBS={cap}")]);
    }
    run.args(&child[1..]);
    let status = run.status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, format!("{} target build failed inside container", plan.distro.name())));
    }
    println!("akt: target package artifacts are in {}", output.display());
    drop(cleanup);
    Ok(())
}
