# akt v0.4.0

## Highlights

- Read-only system inventory and machine-specific build recommendations through the CLI and GUI.
- GUI recommendations and controls for package target, CPU baseline, timer frequency, THP policy, normal compiler jobs, ThinLTO jobs, and source pinning.
- Arch/Debian package-target selection from supported x86-64 Linux hosts using disposable Podman/Docker containers.
- User-writable workspaces separated from root-owned installed configuration.
- Configuration checks that stop before compilation when requested options do not survive Kconfig resolution.
- Prebuilt glibc and static-musl Linux x86-64 binaries and complete standalone bundles; both support Arch and Debian-family hosts.
- Interactive installer modes for standalone, per-user, and system-wide installation.
- A source build can bootstrap a minimal stable Rust toolchain in the user's data directory without editing shell profiles.
- CachyOS source checksums are synchronized from matching .SRCINFO metadata in the checked-out upstream revision. Source integrity verification stays enabled.

## Important limitations

- This release is the kernel builder, not a prebuilt ArxOS kernel. Generated kernel packages depend on configuration, selected source, and hardware/module needs.
- Kernel performance depends on workload and hardware; recommendations are conservative starting points, not benchmark claims or hardware-safety guarantees.
- Arch/CachyOS and Debian kernel recipes have different feature availability. A missing BORE, BBRv3, LTO, real-time, or hardening option must not be treated as enabled unless the selected source verifies it.
- No package should be installed on a daily-driver system before the package has been inspected and the resulting kernel tested in a disposable VM with a bootable fallback available.
- Cross-target builds need Podman or Docker and network access to obtain the target image and build dependencies.
- SHA256SUMS detects changed downloads but is not an independent signature.
