# akt v0.4.1

**Plan without the 60 GiB floor, and verify sources without compiling.** This release adds two ways to check a build before committing hours to it.

## Highlights

- 🧪 **`akt build --dry-run`** is plan-only. It checks the resource plan without requiring the 60 GiB full-build disk floor; available disk is still measured and reported.
- 🔎 **`akt build --prepare-only`** (Arch only) fetches the upstream recipe and source files, synchronizes the active BLAKE2 checksum array from matching upstream `.SRCINFO` metadata, and runs `makepkg` source integrity and PGP-signature verification without compiling a kernel.
- 🔑 The source-preparation CI workflow imports pinned upstream signing-key fingerprints. Signature checks are not disabled.
- ⚙️ GitHub Actions can run a lightweight plan-only dry run, source preparation, or an explicit full kernel build with an optional QEMU boot smoke test.
- 🐛 Fixed the resource-planning distinction between a lightweight dry run and a real kernel build.

## Get it

| Asset | What it is |
|---|---|
| `kernel-builder-v0.4.1-x86_64-linux.tar.gz` | Full bundle (binary, installer, config, patches, GUI). Built on Ubuntu 22.04 for glibc compatibility. |
| `kernel-builder-v0.4.1-x86_64-linux-musl.tar.gz` | Same bundle with a static musl binary. |
| `akt-x86_64-linux` | Bare glibc binary. |
| `akt-x86_64-linux-musl` | Bare static binary. |
| `SHA256SUMS` | Checksums for the four files above. |

```sh
sha256sum --check SHA256SUMS
tar xf kernel-builder-v0.4.1-x86_64-linux.tar.gz
cd kernel-builder && ./install.sh --user
```

Release bundles ship a compiled binary, so no Rust toolchain is needed to install them.

## Validation notes

- Rust compilation and unit tests passed in GitHub Actions for the prepare-only implementation.
- A source-preparation run verified all five active BLAKE2 entries, including the kernel archive, config, DKMS patch, and BORE patch. CI's subsequent PGP/account setup run still failed before successful completion; a successful final prepare-only run is not yet confirmed.
- No full kernel package build or QEMU boot test is claimed by these notes. Those remain separate validation steps and require sufficient runner disk space.

## Important limitations

> [!WARNING]
> Do not install a package from this tool on a daily-driver system until it has been inspected and the resulting kernel has been boot-tested in a disposable VM with a bootable fallback available.

- This release is the kernel builder, not a prebuilt ArxOS kernel. Generated kernel packages depend on configuration, selected source, and hardware/module needs.
- Kernel performance depends on workload and hardware; recommendations are conservative starting points, not benchmark claims or hardware-safety guarantees.
- Arch and Debian kernel recipes have different feature availability. A missing BORE, BBRv3, LTO, real-time, or hardening option must not be treated as enabled unless the selected source verifies it.
- Cross-target builds need Podman or Docker and network access to obtain the target image and build dependencies.
- `SHA256SUMS` detects changed downloads but is not an independent signature.
