# akt v0.4.0

## Highlights

- Read-only machine inventory and machine-aware build recommendations via `akt system` and the GUI.
- GUI **Apply recommendations** action plus controls for package target, CPU baseline, timer frequency, THP policy, compiler job ceiling, ThinLTO backend ceiling, and optional CachyOS source pinning.
- Upstream-backed CachyOS checksum synchronization: conditional scheduler-patch BLAKE2 checksums are taken from the matching variant `.SRCINFO` at the exact recipe commit. Missing or inconsistent metadata fails closed; integrity checks are never bypassed.
- Optional Arch/Debian package-target selection from another supported x86-64 Linux host using a disposable Podman/Docker build container.
- Installer supports system-wide and per-user standalone installation, and offers to bootstrap stable Rust with rustup when installing from source without Cargo.
- Prebuilt portable glibc and static-musl bundles, an installable Debian `.deb`, an Arch `.pkg.tar.zst`, and `SHA256SUMS` are built and checked by CI before publishing.

## Important limitations

- These are releases of the **kernel-builder tool**, not prebuilt ArxOS kernels. You must run a build to create a kernel package.
- Kernel performance depends on workload and hardware; recommendations are starting points, not benchmark claims or hardware safety guarantees.
- Arch/CachyOS and Debian kernel recipes have different feature availability. A missing BORE, BBRv3, LTO, real-time or hardening option must not be treated as enabled unless the selected source can verify it.
- No package should be installed on a daily-driver system before the generated kernel has been inspected and boot-tested in a disposable VM with a bootable fallback available.
- Cross-target builds need Podman or Docker and network access to fetch the target image and build dependencies.
- SHA-256 checksums detect changed downloads but are not independent signatures.
