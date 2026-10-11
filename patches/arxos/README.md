# ARXOS kernel patches

Drop `*.patch` or `*.diff` files here to have them applied to **every** build, on both the Arch and
the Debian path. **This directory currently ships no patches.**

## How patches in this directory are used

| Question | Answer |
|---|---|
| Which files are picked up? | Files ending in `.patch` or `.diff`. Anything else, including this README, is ignored. |
| In what order? | Filename order, so prefix with numbers: `0001-fix-foo.patch`, `0002-bar.diff`. |
| When do they apply? | After the tuning step and **before** the final ARXOS configuration overlay, so the config gate sees the result. |
| What happens on failure? | **Strict by default:** a patch that does not apply aborts the build. Pass `--patch-optional` to skip non-applying patches instead. |
| Which `-p` level? | Detected automatically: `-p1`, then `-p0`. A patch already present in the tree is detected and skipped. |
| Can I verify them? | Yes. Put a `SHA256SUMS` file in this directory; it is checked with `sha256sum -c --strict` before anything is applied, and a mismatch aborts in both modes. |

```sh
cd patches/arxos
sha256sum *.patch > SHA256SUMS      # regenerate whenever the set changes
akt patches                         # list what would be applied, in order
```

## Order across directories

Bundled directories first (`arxos`, then `kali` on the Debian path), then anything passed with
`--patch`, in the order given.

## Cross-target builds

`--target-distro` builds that run in a container do **not** see files placed here: only this README is
embedded in the container build context. Pass such patches explicitly with `--patch <file|directory>`.

## Notes

- This directory is installed alongside the binary (`install.sh` copies it) and is looked up through
  the data directory, so a standalone bundle uses the copy next to its binary.
- Review the build log for `applied`, `already applied` and `DID NOT APPLY` lines before accepting a
  kernel.
