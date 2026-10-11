# Kali and Debian-user kernel patches

Drop `*.patch` or `*.diff` files here to have them applied on the **Debian build path only**.
**This directory currently ships no patches.** Its name does not mean wireless or other features are
bundled: the ARXOS Kconfig overlay (applied on both paths) already enables the in-tree `mac80211`
stack, and out-of-tree DKMS Wi-Fi drivers are not part of this tool.

## How patches in this directory are used

| Question | Answer |
|---|---|
| Which files are picked up? | Files ending in `.patch` or `.diff`. Anything else, including this README, is ignored. |
| In what order? | After everything in `patches/arxos/`, then filename order within this directory. |
| When do they apply? | After the Debian tuning step and **before** the final ARXOS configuration overlay. |
| What happens on failure? | **Strict by default:** a patch that does not apply aborts the build. `--patch-optional` skips non-applying patches instead. |
| Which `-p` level? | Detected automatically: `-p1`, then `-p0`. Already-applied patches are skipped. |
| Can I verify them? | Yes. A `SHA256SUMS` file in this directory is checked with `sha256sum -c --strict` before any patch is applied; a mismatch aborts in both modes. |

```sh
cd patches/kali
sha256sum *.patch > SHA256SUMS
akt patches --target-distro debian      # list what would be applied, in order
```

## Cross-target builds

A `--target-distro debian` build from an Arch-family host runs in a container that does **not** see
files placed here (only this README is embedded). Pass those patches with `--patch <file|directory>`.

## Notes

- Debian source does not automatically contain BORE or BBRv3 patches. If you add such a series here,
  the build verifies the resulting `.config`; a requested option that does not survive Kconfig
  resolution stops the build instead of being reported as present.
- Review the build log for `applied`, `already applied` and `DID NOT APPLY` lines before accepting a
  kernel.
