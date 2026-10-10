# akt v0.4.1

## Highlights

- Plan-only `akt build --dry-run` checks the resource plan without requiring the 60 GiB full-build disk floor; available disk is still measured and reported.
- New Arch-only `akt build --prepare-only` fetches the CachyOS recipe and source files, synchronizes the active BLAKE2 checksum array from matching upstream `.SRCINFO` metadata, and runs makepkg source integrity and PGP-signature verification without compiling a kernel.
- The source-preparation CI workflow imports pinned CachyOS signing-key fingerprints; signature checks are not disabled.
- GitHub Actions supports a lightweight plan-only dry run, source preparation, or an explicit full kernel build with optional QEMU boot smoke test.
- Fixed the resource-planning distinction between a lightweight dry run and a real kernel build.

## Validation notes

- Rust compilation and unit tests passed in GitHub Actions for the prepare-only implementation.
- A source-preparation run verified all five active BLAKE2 entries, including the kernel archive, config, DKMS patch, and BORE patch. CI's subsequent PGP/account setup run still failed before successful completion; a successful final prepare-only run is not yet confirmed.
- No full kernel package build or QEMU boot test is claimed by these notes. Those remain separate validation steps and require sufficient runner disk space.

## Important limitations

- This release is the kernel builder, not a prebuilt ArxOS kernel. Generated kernel packages depend on configuration, selected source, and hardware/module needs.
- Kernel performance depends on workload and hardware; recommendations are conservative starting points, not benchmark claims or hardware-safety guarantees.
- Arch/CachyOS and Debian kernel recipes have different feature availability. A missing BORE, BBRv3, LTO, real-time, or hardening option must not be treated as enabled unless the selected source verifies it.
- No package should be installed on a daily-driver system before the package has been inspected and the resulting kernel tested in a disposable VM with a bootable fallback available.
- Cross-target builds need Podman or Docker and network access to obtain the target image and build dependencies.
- SHA256SUMS detects changed downloads but is not an independent signature.
