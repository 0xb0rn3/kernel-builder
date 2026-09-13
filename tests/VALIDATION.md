# Resource planner validation, 2026-09-13

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
