# kernel-builder v0.1.1

Resource-aware builds for x86-64 Linux.

- Compiler jobs now account for available CPUs, RAM and visible cgroup v2 limits.
- Preserve host headroom, check work-disk space, and bound ThinLTO backend jobs.
- `akt resources --json` exposes the resource plan. `AKT_MAX_JOBS` lowers the cap.
- The Arch and Debian build paths share the same planner; the GUI uses that engine.

Assets include native glibc and static musl binaries, matching installer bundles,
and `SHA256SUMS`. Prefer the musl bundle when the target has an older glibc.
Verify downloads before installation. Checksums are not independent signatures.

Both release formats passed CLI, checksum, archive and actual installer checks
in a disposable QEMU/KVM VM. The resource implementation also has eight unit
and six integration checks. See `tests/VALIDATION.md` for scope and limits.

This release updates the builder, not the ARXOS kernel or ISO. It does not certify
end-to-end kernel builds, runtime safety, performance gains or RF hardware.
Optional patch directories contain no bundled patch series. Existing rolling
recipe and patch-skip behavior still require review for a production kernel.

Extract the chosen bundle and run `sudo ./install.sh`. Run `akt --version` to
confirm `0.1.1`, then `akt resources --json` before starting a kernel build.
