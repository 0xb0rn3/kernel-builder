# Kernel builder validation

## Lock error handling and HTTP checks, 2026-09-14

Host compilation only: `cargo test --no-run --locked --offline`,
`cargo build --release --locked --offline`, and the C fault-injection fixture
compiled with `-O2 -Wall -Wextra -Werror`. No host runtime tests were run.

Execution: disposable QEMU/KVM live guest, UUID
`2193f46f-cd6a-41bb-b29e-9a57d425b36d`, kernel `7.2.5-1-arxos`.
The guest used airootfs; the attached work disk stayed unmounted.
Host and guest artifact hashes matched:

- Patched release binary: `a25f85868c7b85bb0b6b4ce32a521283201162c8bf02cf2fbb1432491c83b4fc`
- Rust test binary: `67006c88448f5d742b05367f62156b573af54db3b9765b74f65c8b0996751fa9`
- Fault fixture: `424c6186deb269228178db3dac1110f8f158ee6a6ea4b045a7bd25c50841bc68`
- Baseline 61a9678 binary: `5ed6fdad672614250c8b7acbae29a06760305734dfa191f98dc638da9b13463f`

Results:

- Rust unit binary: 16 passed, 0 failed, 2.66 seconds, exit 0.
- [hardening-vm.py](hardening-vm.py): 6 passed, 40.519 seconds, exit 0.
- The injected F_SETFD failure test fails on the unchanged 61a9678 baseline:
  it ignores EPERM and reaches resource validation. With the patch, the
  command returns EPERM before recipe preparation and releases the lock.
  Invalid resource settings prevent the baseline from reaching a real recipe.

No package installation, kernel build or destructive toolkit action ran.
The HTTP tests cover initial-line/header deadlines, duplicate security
headers, rejected build requests, normal page delivery, bounded log reads,
and symlink/FIFO/writable-parent refusals. See [procedure](hardening-vm.md).

Not established: a full real bash/makepkg/compiler descendant lifecycle after
worker death, entropy-source failure at GUI startup, real recipe integrity,
kernel compilation/boot, performance, or release-asset acceptance. The v0.3.0
published binaries are unchanged and do not include this follow-up patch.

## Resource planner validation, 2026-09-13

## Binary release v0.1.1

Native glibc and static musl x86-64 Linux release binaries compiled on the host.
The musl binary is static PIE with no ELF interpreter. Both exact binaries and
their installer bundles passed `release-vm.py` in the same disposable VM:
four test methods with both ABI subcases, 1.233 seconds, exit 0.

Checks cover all four asset hashes, safe bundle paths, matching standalone and
bundled binary bytes, version 0.1.1, help, detect/plan/resources JSON, dry-run,
invalid-config refusal, installer syntax, and actual installation of both
bundles. Installed binary/data bytes and desktop entry were verified. No kernel
build or toolkit operation ran in the guest. Shutdown requested after export.

This is CLI and installer validation on one Arch-family VM, not a full GUI,
Debian build, physical hardware or kernel runtime acceptance test.

Repeat with `AKT_RELEASE_DIR=/path/to/assets python3 tests/release-vm.py` as root
inside the named throwaway VM with its work disk mounted at `/mnt/kernelbuild`.

## Resource implementation checks

Host compilation: `cargo build --release` and
`rustc --edition=2021 --test src/resources.rs -o resources-tests` succeeded.
Execution target: disposable QEMU/KVM arxos-throwaway, UUID
`2193f46f-cd6a-41bb-b29e-9a57d425b36d`, not the build host.

- Eight Rust tests passed: CPU/RAM limits, downward cap, low-resource refusal,
  MemAvailable parsing, parent cgroup/high limits, recipe job rewriting,
  and preservation of kernel architecture linker flags.
- Six `resources-vm.py` integration tests passed in 0.069 seconds: native CLI
  inspection/dry-run, invalid caps, execution of capped recipe fixtures,
  real retained kernel Makefile transformation, wrapper/shell checks, and
  byte-identical planner source in the two builders.
- The distribution builder's existing 17 source/backport/config/watcher tests
  passed in 2.293 seconds on the same VM.

The binaries were compiled on the host and transferred with the guest agent.
The VM tests do not build or install a kernel, enroll keys, change authentication,
or exercise destructive toolkit behavior. VM shutdown requested after evidence.

To repeat integration checks, supply `RESOURCE_HELPER`, `AKT_BINARY`, and
`PRIVATE_BUILDER` paths plus `ARXOS_TEST_VM_UUID`. A mounted disposable work
filesystem at `/mnt/kernelbuild` with at least 60 GiB free is required. The
script needs retained source fixtures described by the distribution builder.
Rust unit tests are self-contained and do not require that private checkout.

Not validated here: full Arch/Debian kernel compilation, installation, runtime,
performance or RF hardware. This change is build-resource management only.
