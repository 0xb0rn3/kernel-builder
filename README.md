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
akt build                  # build a tuned kernel for this host
akt build --name mykernel  # rename the kernel: builds linux-mykernel (default: linux-arxos)
akt build --flavor rt      # real-time flavor (PREEMPT_RT + BORE)
akt build --dry-run        # show the plan and check resources without building
akt gate path/to/.config   # verify the hardening delta is present
akt gui                    # local web GUI at http://127.0.0.1:8787
```

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

The September 13 resource-planner update is available in source on `main`.
No new binary release was published with that update. Older release binaries
may not contain `akt resources` or the automatic job limits; build current source
to use these changes now.

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

- `patches/arxos/*.patch`: optional local source patches for both build paths.
- `patches/kali/*.patch`: optional local source patches for the Debian path only.
- `config/arxos.fragment`: the bundled hardening configuration overlay.

The two patch directories currently contain README files only. No additional
wireless, scheduler, or distribution emergency-backport series is bundled there.
The build engine applies supplied patches before its final configuration overlay.

Patches are applied `--forward` and skipped (never fatal) if they don't apply on the current base.

## Status

Development tool. Resource management was added in source commit `91dffc6`.
Host compilation succeeded; eight Rust unit tests and six integration checks
passed in a disposable QEMU/KVM VM. See [test evidence](tests/VALIDATION.md).

This does not establish end-to-end Arch/Debian build success, boot safety,
performance gains, or hardware support. Compile on the host; run regression,
installation and runtime acceptance in a throwaway VM before hardware use.

## License

GPL-3.0-or-later (kernel-adjacent tooling).
