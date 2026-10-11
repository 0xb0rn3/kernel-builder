# Disposable-VM regression checks

The Python scripts in this directory exercise behavior that should never run on a machine you care
about: they start the GUI server, inject syscall failures, install into `/usr`, or read a build
tree. They are **non-destructive by design but VM-only by policy**, and each one refuses to start
unless it is sure it is on the machine you named.

> [!WARNING]
> Compile on the host, execute only inside the identified disposable VM. Take a snapshot first.

## What is here

| File | Needs | What it checks |
|---|---|---|
| `hardening-vm.py` | `AKT_BINARY`, `AKT_DENY_SETFD`, `ARXOS_TEST_VM_UUID` | **Build lock:** when `fcntl(F_SETFD)` is made to fail (so the lock cannot be inherited by build children), `akt build` aborts before touching a recipe and releases the lock. **GUI:** the page serves a per-run token; `POST /api/build` is rejected without the token, with a wrong token, with a foreign `Host` (421) or with `Origin: null`; duplicate `Host`, `Origin` or token headers return 400; `/api/log` is capped at 256 KiB and returns nothing for a symlink, a FIFO or a group/world-writable parent directory; a request line or header that trickles bytes past the deadline gets `408` and the server keeps serving. |
| `deny-setfd.c` | `gcc` | Fixture used by the build-lock test. It installs an unremovable seccomp filter that makes `fcntl(F_SETFD)` fail with `EPERM` for itself and its children only, then `exec`s the command you give it. |
| `release-vm.py` | `AKT_RELEASE_DIR` | Release bundles: `SHA256SUMS` matches, tarball members are safe (no absolute paths, `..`, links or devices), required files are present, `install.sh` passes `bash -n`, the bare and bundled binaries answer `--version`, `--help`, `detect`, `resources`, `plan`, `build --dry-run` and `gate`, and a system install lands in `/usr/local/bin` and `/usr/share/kernel-builder`. |
| `resources-vm.py` | `RESOURCE_HELPER`, `AKT_BINARY`, `PRIVATE_BUILDER`, `ARXOS_TEST_VM_UUID` | Resource planner against a real mounted work disk, rejection of bad job caps, recipe `nproc` rewriting, and ThinLTO Makefile limiting against a retained real kernel Makefile. |

## Running the GUI and lock checks

On the host:

```sh
cargo build --release --locked
gcc -O2 -o /tmp/deny-setfd tests/deny-setfd.c
```

Copy `target/release/akt`, `/tmp/deny-setfd` and `tests/hardening-vm.py` into the VM, then:

```sh
cat /sys/class/dmi/id/product_uuid          # the VM's identity
AKT_BINARY=./akt AKT_DENY_SETFD=./deny-setfd \
ARXOS_TEST_VM_UUID=<that uuid> python3 hardening-vm.py
```

The script exits with `refusing` unless the UUID matches `/sys/class/dmi/id/product_uuid` **and**
`systemd-detect-virt --vm` reports a virtual machine. The server tests bind a random loopback port
and stub `xdg-open`, so no browser opens.

## Running the release and resource checks

`release-vm.py` and `resources-vm.py` additionally compare against a **hard-coded UUID** for the
author's throwaway VM, so they will refuse to run elsewhere until you change that constant to your
own VM's `product_uuid`. Both also carry assumptions tied to the author's environment:

- `release-vm.py` expects `SHA256SUMS` to list four files and compares `--version` against a
  `VERSION` constant at the top of the file (at the time of writing `0.1.1`, while `Cargo.toml` says
  `0.4.1`), so update it to the version you are testing. Its CLI checks run with `AKT_WORK` set to
  `/mnt/kernelbuild`, so that work disk (60 GiB free) must exist here too. Its install check also
  asserts the exact text of the generated `.desktop` entry (`Exec=akt gui`), whereas `install.sh`
  currently writes a quoted absolute path; reconcile the two before relying on that check.
- `resources-vm.py` needs a work disk mounted at `/mnt/kernelbuild` with at least 60 GiB free, and a
  private ARXOS builder checkout (`PRIVATE_BUILDER`) for its source-identity and retained-Makefile
  tests. Those tests cannot pass without that checkout.

Unit tests need no VM: `cargo test --locked`.
