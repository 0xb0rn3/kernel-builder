<div align="center">

# ARXOS Kernel Tuner

**Machine-aware kernel builds for Arch and Debian targets, in one tool.**

Hardware inventory · Performance tuning · Kernel hardening · Resource-aware builds · Local GUI

<sub>An <a href="https://arxos.uk">ArxOS</a> tool by <a href="https://stingraylabs.pages.dev">Stingray Labs</a>.</sub>

</div>

## What it does

`akt` detects the running machine and recommends conservative starting points based on CPU
features, available/logical CPUs, RAM, cgroup limits, free build disk, exposed thermal sensors,
CPU-frequency policy, and battery/AC state. Missing telemetry is reported as unknown rather
than guessed. Hardware recommendations are advisory, not a hardware safety guarantee.

- **Arch-family target**: builds from the CachyOS `linux-cachyos` PKGBUILD, applies the ArxOS
  configuration and patches, and creates a pacman package plus headers. The recipe supplies
  CachyOS-specific tuning where the selected upstream revision supports it.
- **Debian-family target**: starts from Debian kernel source, applies the ArxOS configuration and
  local patches, then produces `.deb` image/header packages. Debian source does not automatically
  contain BORE or BBRv3 patches; unsupported requested features must be visible and verified,
  not described as present merely because a config command was attempted.
- **Cross-target builds**: `--target-distro arch` or `--target-distro debian` can select the other
  package family on an x86-64 Linux host using Podman or Docker. The target builder runs in a
  disposable container and writes package artifacts to the selected output directory; this
  avoids installing the target distro's build dependencies onto the host. It is still a Linux
  container build, not a virtual machine or cross-architecture compiler.
- **Source traceability**: Arch builds report the exact CachyOS source commit. --source-ref
  can select a CachyOS tag, branch, or commit; leave it unset for the rolling source path.
- **Upstream checksum synchronization**: before source preparation, akt asks makepkg --printsrcinfo
  for the active source URLs and rebuilds the PKGBUILD BLAKE2 checksum array using matching .SRCINFO
  metadata committed alongside that same CachyOS revision. This follows upstream patch and checksum
  changes without trusting a downloaded file's own digest. Missing, mismatched, or ambiguous
  metadata stops the build; makepkg integrity verification is never skipped.

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
akt detect                                      # host distro + native package path
akt system                                      # hardware inventory + per-machine recommendations
akt system --json                              # machine-readable inventory/recommendations
akt resources --json                           # CPU/RAM/cgroup/disk build-resource plan
akt explain                                     # explain performance/security settings
akt plan                                        # show the host-native plan, no build
akt plan --target-distro debian                 # inspect a Debian package plan from any supported host
akt patches                                     # list patches applied by the selected target
akt build                                       # build for the host distribution
akt build --target-distro debian                # build a .deb target from an Arch-family host (Podman/Docker)
akt build --target-distro arch                  # build a pacman target from a Debian-family host (Podman/Docker)
akt build --cpu-target portable --max-jobs 4    # portable CPU target + cap normal compile jobs
akt build --max-jobs 4 --thinlto-jobs 2          # independently cap ThinLTO backends
akt build --hz 500 --hugepages madvise          # selectable timer rate and THP default policy
akt build --source-ref <tag-or-commit>          # pin the Arch/CachyOS recipe source

`--dry-run` is plan-only: it reports actual disk capacity and checks CPU/RAM planning without imposing
the 60 GiB full-build disk floor. `--prepare-only` is for an Arch-family host and runs the CachyOS
source download, BLAKE2 checksum checks, and PGP signature verification, then exits before configuration
or compilation. Neither mode installs a kernel.
akt build --performance-governor                # opt-in to performance governor as kernel default
akt build --patch p.patch                       # apply an additional patch; repeatable, fail-fast
akt build --name mykernel                       # build linux-mykernel instead of linux-arxos
akt build --flavor rt                           # request the real-time profile (source support is required)
akt build --dry-run                             # show plan and inspect resources without compiling
akt build --prepare-only                        # Arch only: fetch sources and verify BLAKE2 + PGP; no compile
akt rollback                                    # reinstall the previous host kernel package
akt gate path/to/.config                        # verify the hardening delta
akt gui                                         # local web GUI at http://127.0.0.1:8787
```

## Machine inventory and tuning recommendations

Run `akt system` before building. It reports the OS, machine model when exposed by DMI, CPU model
and instruction flags, logical/physical core estimate, total and available RAM, swap, CPU-frequency
driver/governor, reported maximum frequency, exposed thermal zones, DRM GPU devices, battery/AC
status, work-filesystem free space, and the resource planner's current job ceilings. Use
`akt system --json` for automation. Sensor access differs by firmware and kernel; a missing reading
is shown as unknown. Temperature is a point-in-time reading and is not predictive thermal control.

The command and GUI provide a one-click **Apply recommendations** action. It chooses an initial CPU
target from exposed CPU flags, recommends conservative make/ThinLTO job caps, leaves the performance
governor opt-in, and defaults THP to `madvise`. On battery-powered machines, the GUI suggests 500 Hz
for the CachyOS/Arch target, which supports intermediate tick rates, and 250 Hz for Debian source to
stay with its standard Kconfig choices; on AC/no-battery detection it suggests 1000 Hz. Treat this
as a benchmark starting point, not a claim that one rate is universally faster or more efficient.
The build gate rejects any requested rate that does not survive the selected source's configuration.

| Setting | Behavior | Trade-off / guardrail |
|---|---|---|
| CPU baseline | `auto` selects the highest detected x86-64 ISA level; choose `portable` or `v1` for broad compatibility, or explicitly choose `v2`, `v3`, `v4` | Higher ISA targets can make a package fail to boot on older CPUs. `auto` is machine-specific, not portable. |
| Timer rate (`--hz`) | Select 100, 250, 300, 500, 600, 750 or 1000 Hz | Higher is not automatically faster; benchmark latency, power and throughput on representative workloads. |
| THP (`--hugepages`) | `madvise` by default, or `always` | `madvise` is a conservative general-purpose starting point; test memory-intensive workloads before changing it. PREEMPT_RT may constrain available options. |
| Normal compiler jobs (`--max-jobs`) | `auto` or positive integer; still bounded by CPU/RAM/cgroup/disk planning | Start lower on thermally constrained laptops; resource detection is a snapshot, not active temperature control. |
| ThinLTO backends (`--thinlto-jobs`) | Independent `auto` or positive integer cap | Link-time optimization can consume substantial memory; lower this separately if final linking is the bottleneck. |
| Performance governor | Opt-in using `--performance-governor` | Changes the kernel's default governor choice where supported; it does not itself configure firmware or guarantee higher clocks. Usually leave disabled for battery-oriented laptops. |
| CachyOS source ref | Optional `--source-ref REF` on Arch target | Record/pin a reviewed tag or commit for repeatable recipe inputs; a rolling build can change as upstream changes. |

Example for an ArxOS/Arch laptop where portability matters more than a machine-specific ISA:

```sh
akt system
akt build --cpu-target portable --max-jobs 4 --thinlto-jobs 2 --hz 500 --hugepages madvise
```

The ISA recommendation only uses instruction flags visible to the current Linux environment. The
tool does not overclock the CPU, undervolt it, change firmware power limits, or continuously throttle
a build in response to temperature. Always test performance changes against a known baseline.

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

Build parallelism is calculated from available CPUs, affinity/quotas, available RAM, visible
cgroup v2 ancestor limits, and free disk on the workspace filesystem. The planner leaves one logical
CPU where possible, reserves host memory, budgets 2 GiB per compiler job, and caps ThinLTO backends
at four by default. Swap is not counted. A separate `AKT_MAX_LTO_JOBS` / `--thinlto-jobs` cap
controls ThinLTO backends independently of `AKT_MAX_JOBS` / `--max-jobs`.

`--max-jobs N` and `--thinlto-jobs N` are upper bounds: the detected resource plan can lower them.
Use `auto` to return to the planner default. The GUI exposes both caps and the machine-recommended
starting values. Detection runs before preparation and again before compilation; it is a snapshot,
not an OOM guarantee or continuous temperature controller. Large kernel builds may need more than
the 60 GiB minimum free-space floor. Unsupported/unreadable cgroup-v2 layouts fail with an
explanation rather than guessing a larger job count.

The Arch recipe's job flags and Debian make invocation use the calculated normal compile-job cap.
ThinLTO flags are appended to the prepared kernel Makefile without replacing architecture linker
flags. Unknown recipe or ThinLTO layouts stop for review. Workspaces default to a user-writable
cache directory, separate from root-owned configuration under `/usr/share/kernel-builder`.
Builds share a work-directory lock which remains held by build subprocesses.

```sh
akt system --json
AKT_WORK=/path/to/kernel-work akt resources --json
AKT_WORK=/path/to/kernel-work akt build --max-jobs 4 --thinlto-jobs 2 --dry-run
AKT_WORK=/path/to/kernel-work akt build --max-jobs 4 --thinlto-jobs 2
```

The work filesystem must exist or have an existing parent with enough free space. Resource planning
currently requires Linux cgroup v2. These checks do not certify compile success, runtime safety, or
performance gains.

## GUI

`akt gui` starts a loopback-only web UI with a live build log. It shows the hardware inventory and
conservative recommendations, provides **Apply recommendations**, and exposes flavor, package
target, CPU baseline, timer rate, THP policy, normal compiler jobs, ThinLTO jobs, optional source
pin, package name/revision and output location. These controls flow into the same plan/build engine
as the CLI. The GUI has no telemetry and uses Google Fonts externally; disable network access or
replace the font imports if a fully offline interface is required. Cross-target builds use the same
Podman/Docker path as the CLI.

### Install from a source checkout

~~~sh
git clone https://github.com/0xb0rn3/kernel-builder
cd kernel-builder
chmod +x install
./install
~~~

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

Development tool. The app includes read-only machine inventory, tuning recommendations,
source-metadata-based CachyOS checksum synchronization, an interactive standalone/user/system
installer, prebuilt Linux releases for glibc and musl, and disposable-container cross-target builds.

CI validates Rust compilation, unit tests, and release packaging. End-to-end Arch/Debian kernel
package builds and boot tests remain outstanding for this feature set. Source-specific Kconfig
support, container permissions, generated package contents, installer behavior, and runtime boot
still require testing. Do not install a new kernel on a daily-driver host until the package has
been inspected and boot-tested in a disposable VM.

For existing lock-error and local HTTP regression checks, see
[the non-destructive VM procedure](tests/hardening-vm.md). Compile test binaries on the host, then
execute integration and runtime acceptance only inside the identified disposable VM.

Passing a config gate is not security certification. The tool cannot promise performance gains or
support for every device; benchmark against a known kernel and keep a bootable fallback available.

## Packaging and publishing a release

For a local, non-publishing bundle build:

```sh
cargo test --locked
cargo build --release --locked
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl
bash tools/package-release.sh /path/to/new-output-directory
(cd /path/to/new-output-directory && sha256sum --check SHA256SUMS)
```

The helper refuses an existing output directory and packages only explicit runtime files. It does
not install or upload anything itself. The release workflow runs it, verifies all sums, and publishes
a new release when the Cargo version has not been released yet; it will not replace existing assets.
For planned releases, bump the Cargo version and update RELEASE_NOTES.md. The glibc bundle is built on
Ubuntu 22.04 for compatibility and the musl bundle is static; both run the builder on supported
x86-64 Arch and Debian-family hosts.

## License

GPL-3.0-or-later (kernel-adjacent tooling).
