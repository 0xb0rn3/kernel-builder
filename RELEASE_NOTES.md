# akt v0.5.3

**Wireless injection and software-defined radio in every ArxOS kernel, and a CI kernel job that runs again.** This release follows v0.5.3.

## Highlights

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
