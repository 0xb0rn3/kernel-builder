# akt v0.5.6-1

**Only a protection stops the build.** v0.5.6 refused to compile when any line of the ArxOS config fragment did not survive Kconfig, so an option renamed or dropped upstream, even a touchscreen driver, would have held back a kernel security update until someone edited the fragment. Now a section of the fragment is marked `[required]` when it holds protections or what AnonKit needs (hardening, disk crypto, anonymity, netfilter, address privacy): a drop there still stops the build. Any other drop is printed as a notice and the build goes on. `akt fragment <config> [--flavor F]` lists both kinds for a built configuration.

## Earlier: v0.5.6

**A config option that Kconfig drops now stops the build, and the gaming flavor leaves the CPU governor to you.** This release follows v0.5.4.

## Highlights

- 🧱 **Every line of the ArxOS config fragment must survive.** Kconfig drops an option without a word when a dependency is missing or the option no longer exists upstream. akt now compares the resolved configuration with the fragment, line by line, before it compiles, and stops with the line number and the value that was built. The check found four stale lines (one option removed upstream, two that can only be modules) and one driver that was patched in but never built.
- 🖐️ **The D-WAV USB multitouch driver is built.** `patches/arxos/0003` added the driver in v0.5.3, but the fragment never enabled it, so no ArxOS kernel carried it. It is now a module.
- 🎮 **`zen`, the gaming flavor** (new in v0.5.5): huge pages always, NTSYNC built in for Windows games under Wine and Proton, sched-ext kept so a scheduler can be swapped in at run time. From this release it no longer builds in the performance governor, which costs battery all day on a laptop; ArxOS offers that as a Control Center switch, and `--performance-governor` still builds it in for anyone who wants it.
- 📡 The real-time flavor cannot have the legacy iptables tables: upstream builds them only without PREEMPT_RT. Its NAT and masquerade come from nftables, which the check still requires.

## Earlier: v0.5.4

**Kernels build themselves when upstream releases, and three build failures are closed.**

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
| `kernel-builder-v0.5.6-x86_64-linux.tar.gz` | Full bundle (binary, installer, config, patches, GUI). Built on Ubuntu 22.04 for glibc compatibility. |
| `kernel-builder-v0.5.6-x86_64-linux-musl.tar.gz` | Same bundle with a static musl binary. |
| `akt-x86_64-linux` | Bare glibc binary. |
| `akt-x86_64-linux-musl` | Bare static binary. |
| `SHA256SUMS` | Checksums for the four files above. |

```sh
sha256sum --check SHA256SUMS
tar xf kernel-builder-v0.5.6-x86_64-linux.tar.gz
cd kernel-builder && ./install.sh --user
```

Release bundles ship a compiled binary, so no Rust toolchain is needed to install them.

## Validation notes

- Unit tests pass with `cargo test --locked`.
- The fragment check was run against the configurations inside the published linux-arxos and linux-arxos-rt 7.3.4-1 packages: after the fragment fixes, the only difference left in each is the D-WAV driver, which those kernels predate.
- The driver lands with the next ArxOS kernel build of each flavor.

## Important limitations

> [!WARNING]
> Do not install a package from this tool on a daily-driver system until it has been inspected and the resulting kernel has been boot-tested in a disposable VM with a bootable fallback available.
