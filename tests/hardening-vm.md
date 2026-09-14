# Lock and HTTP regression checks

Compile on the host without executing the tests:

```sh
cargo test --no-run --locked --offline
cargo build --release --locked --offline
cc -O2 -Wall -Wextra -Werror tests/deny-setfd.c -o target/deny-setfd
```

Copy the printed Rust test executable, target/release/akt, target/deny-setfd,
and tests/hardening-vm.py to a disposable Arch-family VM. Record the source
revision and SHA256 of each artifact; verify the hashes again inside the VM.
No installation is required. The Rust tests need pacman/vercmp from the guest.

Execute the Rust test binary with `--test-threads=1` inside the VM. Then run:

```sh
ARXOS_TEST_VM_UUID=THE_VERIFIED_DISPOSABLE_VM_UUID \
AKT_BINARY=/path/in/guest/akt \
AKT_DENY_SETFD=/path/in/guest/deny-setfd \
python3 /path/in/guest/hardening-vm.py
```

The Python runner refuses a missing/mismatched UUID or a non-VM target. Read
access to /sys/class/dmi/id/product_uuid is required. It uses temporary files
and a loopback listener, suppresses browser launch, and cleans up its server.
It never submits an accepted build request or installs a kernel. Its two
deadline cases take about 40 seconds combined. Do not run it on the host.

Coverage:

- Real F_SETFD failure injected by a process-local seccomp filter. The command
  must abort before recipe preparation and release the acquired lock. An
  invalid resource cap also prevents the old implementation reaching a recipe.
- Normal page response and refusal of missing/wrong tokens and bad origins/hosts.
- Duplicate Host, Origin and X-Akt-Token rejection.
- Bounded log reads; symlink, FIFO and writable-parent refusals.
- Absolute deadlines on both a slowly delivered request line and header.

The Rust tests cover invalid-descriptor errors, descriptor flag preservation,
lock exclusivity, and a surviving sleep child retaining the lock. These checks
do not establish the lifecycle of every real bash/makepkg/compiler descendant.
That acceptance case, entropy-failure startup, real recipe verification, full
kernel compilation and kernel boot tests remain separate. Export evidence and
shut down the claimed VM after testing. Test results belong in VALIDATION.md.
