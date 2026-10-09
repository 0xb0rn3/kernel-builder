# akt v0.4.0

## Highlights

- Read-only system inventory and machine-specific build recommendations via `akt system` and the GUI.
- GUI **Apply recommendations** action plus controls for package target, CPU baseline, timer frequency, THP policy, compiler job ceiling, ThinLTO backend ceiling, and optional source pinning.
- Optional Arch/Debian package-target selection from another supported x86-64 Linux host using a disposable Podman/Docker build container.
- User-writable default build workspaces separated from root-owned installed configuration.
- Configuration checks that stop before compilation when required options do not survive Kconfig resolution.
- Release bundles for glibc and static musl, each accompanied by `SHA256SUMS`.

## Important limitations

- This release is the **kernel builder**, not a prebuilt ArxOS kernel.
- Kernel performance depends on the workload and hardware; the recommendations are conservative starting points, not benchmark claims or hardware safety guarantees.
- Arch/CachyOS and Debian kernel recipes have different feature availability. A missing BORE, BBRv3, LTO, real-time or hardening option must not be treated as enabled unless the selected source can verify it.
- No package should be installed on a daily-driver system before the package has been inspected and the resulting kernel tested in a disposable VM with a bootable fallback available.
- Cross-target builds need Podman or Docker and network access to obtain the target image/build dependencies.
- SHA-256 checksums detect changed downloads but are not independent signatures.
