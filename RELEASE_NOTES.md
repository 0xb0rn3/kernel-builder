# akt v0.5.4

**Kernels build themselves when upstream releases, and three build failures are closed.** This release follows v0.5.3.

## Highlights

- 🤖 **Automatic kernel builds.** A scheduled job asks `akt upstream` which version a build would produce now; when that version has no release yet, `linux-arxos` and `linux-arxos-rt` are built with the release profile on GitHub runners, both pinned to one source commit, and published with their SHA-256 sums.
- ✅ **A package is released only after it boots clean.** Each build passes the hardening gate on the configuration inside the package, then a real boot that must reach userspace with taint 0, lockdown in integrity mode and no warning in the kernel log.
- 🩹 **A false W^X warning no longer taints the kernel.** A 7.3.x stable update made early boot warn about the temporary pages of the ITS mitigation on Intel CPUs and mark the kernel tainted. `patches/arxos/0004` carries the fix posted upstream until a stable release has it.
- 🔁 **A dropped source download resumes.** A connection reset in the middle of a large source used to fail the build; it is retried up to eight times, continuing where it stopped. A checksum or signature that does not match still stops at once.
- 🛑 **No build during a package update.** akt refuses to start while a package transaction is running: a system update that replaces a tool mid-compile fails the build hours in.
- `akt upstream [--json]` prints the kernel version a rolling build would produce, without downloading sources.

## Earlier: v0.5.3

- 📡 **Injected frames are sent as they were built.** `patches/arxos/0001` stops mac80211 renumbering the sequence of an injected frame and lets it go out without waiting for an acknowledgement, which is what frame-injection tools expect on top of the kernel's native injection support.
- 🛰️ **Software-defined radio receivers.** The config fragment now builds the HackRF, Airspy, MSI2500 with its MSi001 tuner and RTL2832 SDR drivers, all as modules, so they cost nothing until a radio is attached.
- 🔍 **carl9170 sniffer mode.** `patches/arxos/0002` turns on promiscuous capture for carl9170 adapters.
- 🖐️ **D-WAV USB multitouch.** `patches/arxos/0003` adds the D-WAV touch panel driver.
- Each new patch is listed in `patches/arxos/SHA256SUMS` and checked before it is applied.

## Fixes

- 🐛 The kernel job in CI failed on every push since v0.4.1: inside the Arch container, `runuser` rejected the freshly created build account. The job now drops privileges with `sudo -u`.

## Get it

| Asset | What it is |
|---|---|
| `kernel-builder-v0.5.3-x86_64-linux.tar.gz` | Full bundle (binary, installer, config, patches, GUI). Built on Ubuntu 22.04 for glibc compatibility. |
| `kernel-builder-v0.5.3-x86_64-linux-musl.tar.gz` | Same bundle with a static musl binary. |
| `akt-x86_64-linux` | Bare glibc binary. |
| `akt-x86_64-linux-musl` | Bare static binary. |
| `SHA256SUMS` | Checksums for the four files above. |

```sh
sha256sum --check SHA256SUMS
tar xf kernel-builder-v0.5.3-x86_64-linux.tar.gz
cd kernel-builder && ./install.sh --user
```

Release bundles ship a compiled binary, so no Rust toolchain is needed to install them.

## Validation notes

- Unit tests pass with `cargo test --locked`.
- A `akt build --configure-only` run applied all three patches (checked against SHA256SUMS), kept all six SDR options through `olddefconfig`, and passed the configuration gate.
- The kernels built from this configuration land with the next ArxOS kernel build; until then the published ArxOS kernels predate these patches.

## Important limitations

> [!WARNING]
> Do not install a package from this tool on a daily-driver system until it has been inspected and the resulting kernel has been boot-tested in a disposable VM with a bootable fallback available.
