<div align="center">

# ARXOS Kernel Tuner

**A tuned, hardened kernel on Arch or Debian, in one tool.**

Performance tuning · Kernel hardening · Resource-aware builds · Local GUI

<sub>An <a href="https://arxos.uk">ArxOS</a> tool by <a href="https://stingraylabs.pages.dev">Stingray Labs</a>.</sub>

</div>

## What it does

`akt` orchestrates tuned kernel builds on supported Arch-family and Debian-family
Linux hosts. It checks required configuration before the long compile. A passing
configuration check is not runtime or security certification.

- **Arch family** (Arch, CachyOS, EndeavourOS, ...): forks the CachyOS `linux-cachyos` recipe
  (BORE, Clang ThinLTO, BBRv3, 1000 Hz, x86-64-v3), overlays the ARXOS config fragment + patches,
  and builds a pacman package `linux-arxos` (+ headers).
- **Debian family** (Debian, Ubuntu, Kali): takes Debian kernel source, requests
  tuning and hardening options, accepts optional local patches, and builds
  `linux-image-*-arxos.deb` (+ headers). Available options depend on the source;
  a configuration request does not add a missing scheduler or networking patch.

The hardening overlay requests allocation/free memory initialization, disabled
hibernation, strict IOMMU defaults, early lockdown LSM, IMA, livepatch capability,
and restricted kernel logs. These controls have platform and policy limits;
they do not guarantee protection against every form of memory capture.

## The config gate (why this exists)

`olddefconfig` can silently revert fragment options, which once shipped a "hardened" kernel that
quietly wasn't. So `akt` re-asserts the delta with `scripts/config` and **hard-verifies it in
`.config` at the config stage** — a missing option aborts the build in seconds instead of after a
multi-hour compile. `akt gate <config>` runs the same check standalone.

## Usage

```sh
akt detect                 # host distro + which build path applies
akt resources --json       # CPU/RAM/disk plan, using AKT_WORK or current directory
akt explain                # what each option means for your custom kernel
akt plan                   # print the tune plan (base, tunes, patches), no build
akt patches                # list every patch that will be applied, in order
akt build                  # build a tuned kernel for this host
akt build --patch p.patch  # also apply your own patch (file or dir); repeatable, fail-fast
akt build --name mykernel  # rename the kernel: builds linux-mykernel (default: linux-arxos)
akt build --flavor rt      # real-time flavor (PREEMPT_RT + BORE)
akt build --dry-run        # show the plan and check resources without building
akt rollback               # reinstall the previous kernel package from prev/ (see Rollback below)
akt gate path/to/.config   # verify the hardening delta is present
akt gui                    # local web GUI at http://127.0.0.1:8787
```

## Rollback

Each build archives the version it supersedes into `prev/` next to the new package. To go back to it:

```sh
akt rollback                 # reinstall the newest linux-arxos package from prev/
akt rollback --name mykernel # roll back a custom-named kernel
```

Why a saved kernel image is not enough on its own: a same-name kernel upgrade (`pacman -U` of
`linux-arxos` over an older `linux-arxos`) **removes the previous version's `/usr/lib/modules/<rel>`
tree**, because those files are owned by the package. If you keep only the old `vmlinuz` + initramfs
and select it in GRUB, it boots but cannot load its modules (e.g. `modprobe vfat` fails, `/boot/efi`
won't mount) and drops to emergency. The reliable rollback is to **reinstall the retained previous
package** (which restores the module tree and rebuilds the initramfs via pacman hooks) — that is what
`akt rollback` does. For a kernel you want to keep permanently alongside another, build it under a
distinct `--name` so the two packages (and module trees) coexist. Boot-test any rollback in a
throwaway VM first.

Flavors: `default` (BORE + ThinLTO), `rt` (PREEMPT_RT), `hardened` (KSPP + kCFI). Name your kernel
with `--name NAME` (builds `linux-NAME`; default `arxos`), so it is yours, not branded. `akt explain`
describes every performance and hardening option that goes into the build, in plain English, before
you commit to a long compile. Add `--json` to `detect` / `plan` / `explain` / `gate` for scripting.

## Automatic build resources

Both build paths size jobs from available CPU parallelism (including affinity and
CPU quotas) and available RAM. The planner also checks visible cgroup v2 memory
limits at every ancestor and requires 60 GiB free on the work filesystem.
It leaves one logical CPU when possible, reserves up to 4 GiB RAM, budgets
2 GiB per compiler job, and caps ThinLTO backends at four. Swap is not counted.
Use `AKT_MAX_JOBS=2 akt build` to lower the cap; it cannot exceed detection.
`AKT_WORK=/path/to/work akt resources --json` inspects that filesystem.
Detection runs before preparation and again before compilation. It is a snapshot,
not an OOM guarantee or continuous load controller. Large kernels can need more
than the disk floor. Unsupported cgroup layouts fail with an explanation.

The Arch recipe's explicit job flags and Debian make command use the computed
limit. ThinLTO flags are appended to the prepared kernel Makefile without replacing
architecture linker flags. Unknown recipe or ThinLTO layouts stop for review.
The planner is std-only Rust, embedded in `akt`, and shared with the distribution
builder. The GUI uses this same build engine. `akt plan` describes the feature
plan; `akt resources` inspects resource limits without starting a build.

```sh
AKT_WORK=/path/to/kernel-work akt resources --json
AKT_WORK=/path/to/kernel-work AKT_MAX_JOBS=2 akt build --dry-run
AKT_WORK=/path/to/kernel-work AKT_MAX_JOBS=2 akt build
```

The work filesystem must exist or have an existing parent with enough free space.
Automatic resource detection currently requires Linux cgroup v2.

This resource update does not certify the existing rolling recipe, patch-skip
policy, package output cleanup, Debian kernel build, or resulting kernel runtime.

## GUI

`akt gui` starts a loopback-only web UI (no telemetry, no external calls beyond Google Fonts) that
detects the host, shows the resolved tune plan for the chosen flavor, and runs the build with a live
log. It matches the ArxOS Control Center design language.

## Install

### Prebuilt binary release (no compiler needed)

Resource controls are included in **v0.1.1** for x86-64 Linux, in both the native
glibc and static musl builds. Earlier binaries do not contain this update.
Each release includes the standalone binaries, bundles with installer/data,
and `SHA256SUMS`. Download the checksum file alongside the assets and verify
the selected files before extraction or installation.

Grab the latest build from **[Releases](https://github.com/0xb0rn3/kernel-builder/releases)**:

```sh
tar xzf kernel-builder-*-x86_64-linux.tar.gz
cd kernel-builder
sudo ./install.sh          # installs the prebuilt akt binary + tunes, patches, GUI
```

The release also ships the bare `akt` binary on its own, enough for `akt detect` / `plan` /
`explain` / `gui` (the GUI is embedded in the binary). For maximum portability across any Linux
(any glibc), use the **`-musl`** static build (`akt-x86_64-linux-musl`, or the `*-musl.tar.gz`
bundle); the plain build is smaller but needs a recent glibc.

To fetch and verify the portable bundle using GitHub CLI:

```sh
mkdir kernel-builder-v0.1.1-download
cd kernel-builder-v0.1.1-download
gh release download v0.1.1 --repo 0xb0rn3/kernel-builder \
  --pattern kernel-builder-v0.1.1-x86_64-linux-musl.tar.gz --pattern SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
tar xzf kernel-builder-v0.1.1-x86_64-linux-musl.tar.gz
cd kernel-builder
./akt --version
sudo ./install.sh
```

Checksums detect changed downloads; they are not an independent signature.

### From source (manual compile)

```sh
git clone https://github.com/0xb0rn3/kernel-builder
cd kernel-builder
cargo build --release     # compile on the host, without root
sudo ./install.sh          # install the freshly built binary + data
```

Compiling needs `rust`/`cargo`. Building a kernel then needs the host's toolchain (Arch: `base-devel`
`clang` `lld` `llvm`; Debian: `build-essential fakeroot libncurses-dev bison flex libssl-dev libelf-dev`).

## Adding patches

Two ways to feed patches into a build:

- **Bundled** — drop `*.patch` / `*.diff` files into a patch directory beside the builder:
  - `patches/arxos/` — applied on every host (after the config overlay).
  - `patches/kali/` — applied on the Debian path only (wireless injection, extra HID, ...).
  Both directories currently ship a README only; no wireless, scheduler, or emergency-backport
  series is bundled by default.
- **Your own, at build time** — `--patch <path>` (repeatable). The path can be a single
  `.patch`/`.diff` file or a directory of them. Your patches apply *after* the bundled ones,
  in the order given:

```sh
akt build --patch fixes/CVE-2026-1234.patch      # one file
akt build --patch ~/my-kernel-patches/           # a whole directory (sorted)
akt build --patch a.patch --patch b/ --patch c.diff
akt patches --patch ~/my-kernel-patches/         # list what WOULD be applied, in order
```

- `config/arxos.fragment` — the Kconfig overlay (the hardening delta). Edit here to change the tune.
  Patches apply before this final configuration overlay.

**How patches are applied**

- Auto-detected at `-p1` then `-p0`; a patch already present in the tree is detected and skipped.
- **Strict by default**: a patch that does not apply **aborts the build** — so a kernel is never
  silently shipped without a fix you expected. Pass `--patch-optional` to skip non-applying
  patches instead (the old lenient behavior).
- **Integrity**: if a patch directory contains a `SHA256SUMS` file, it is verified with
  `sha256sum -c --strict` before any patch is applied; a mismatch aborts regardless of mode.
- Run `akt patches` (add `--json` for machine output) to see the exact ordered list first.

## Status

Development tool. Resource management was added in source commit `91dffc6`.
Host compilation succeeded; eight Rust unit tests and six integration checks
passed in a disposable QEMU/KVM VM. See [test evidence](tests/VALIDATION.md).

This does not establish end-to-end Arch/Debian build success, boot safety,
performance gains, or hardware support. Compile on the host; run regression,
installation and runtime acceptance in a throwaway VM before hardware use.

## Packaging a release

Compile native and `x86_64-unknown-linux-musl` release targets on the build host,
then run `bash tools/package-release.sh /path/to/new-output-directory`.
The helper refuses an existing output directory, packages only explicit runtime
files, and creates checksums. It does not install, upload, or publish anything.
Validate the exact assets in throwaway with `tests/release-vm.py` before publishing.

## License

GPL-3.0-or-later (kernel-adjacent tooling).
