# akt v0.5.2

**Build the stable series as well as mainline, install without losing the running kernel, and ship packages that describe themselves plainly.** This release covers everything since v0.4.1 (v0.5.0, v0.5.1 and v0.5.2).

## Highlights

- 🌿 **`--series stable --kver X.Y.Z`** builds an older X.Y line at its current stable release. akt takes the last tuned recipe of that line and brings it forward with the kernel.org incremental stable updates. Each update is used only when kernel.org's checksum list carries a valid signature from its pinned checksum key, checked in a throwaway keyring, and the file matches that signed list. A mainline and a stable build have separate workspaces and locks, so both can run at the same time.
- 🧾 **`--configure-only`** stops after the configuration gate, before compiling, so you can check a build's final configuration in minutes.
- 📦 **`akt install`** puts a built kernel into the running system and keeps the kernel you are running. If no package file exists for the running version, akt first rebuilds one from the file list pacman records, and after the install it checks that the running module tree is still present.
- 🏷️ **Plain package metadata.** akt rewrites only `.PKGINFO` and `.BUILDINFO`: a plain description, the akt URL, no other kernel in `provides` or `replaces`, and `/build` instead of your build directory. A package is replaced only when every payload file is byte-identical.
- 🕒 **Real-time builds keep their name.** A real-time build is now `linux-<name>-rt`, never `linux-<name>-rt-gcc`. akt reads the package name makepkg will produce before it downloads anything and stops if it is not exactly `linux-<name>`.
- 📜 **The GUI follows the whole build.** The log streams from the first line, with a follow toggle, a copy button and a resizable window. The GUI also has Install, Rollback, Resources, Patches and Gate, plus `--patch`, `--patch-optional` and `--prepare-only`.
- 🛡️ **Network privacy in the config fragment.** Link-layer MAC rewrite and filtering, NAT and redirect for a transparent proxy, netfilter packet logging, SYN cookies, reverse-path filtering and IPv6 address privacy, plus the runtime sysctl drop-in `config/sysctl/90-arxos-network-anon.conf`.

## Fixes

- 🐛 x86-64-v3 detection: Linux reports the LZCNT bit as `abm`, not `lzcnt`, so every v3-capable machine was offered `x86-64-v2`. akt now accepts either spelling.
- 🐛 A cached download from an older series could shadow a new major's download of the same name and fail the integrity check. akt now removes a cached download whose checksum no longer matches the recipe.
- 🐛 A rolling build could silently keep an old recipe after a pinned build left the checkout detached. Rolling builds now fetch the tip and check it out detached.
- 🐛 A multi-line value made the GUI's JSON responses invalid, so the live log stopped updating. Control characters are now escaped.

## Get it

| Asset | What it is |
|---|---|
| `kernel-builder-v0.5.2-x86_64-linux.tar.gz` | Full bundle (binary, installer, config, patches, GUI). Built on Ubuntu 22.04 for glibc compatibility. |
| `kernel-builder-v0.5.2-x86_64-linux-musl.tar.gz` | Same bundle with a static musl binary. |
| `akt-x86_64-linux` | Bare glibc binary. |
| `akt-x86_64-linux-musl` | Bare static binary. |
| `SHA256SUMS` | Checksums for the four files above. |

```sh
sha256sum --check SHA256SUMS
tar xf kernel-builder-v0.5.2-x86_64-linux.tar.gz
cd kernel-builder && ./install.sh --user
```

Release bundles ship a compiled binary, so no Rust toolchain is needed to install them.

## Validation notes

- Unit tests pass with `cargo test --locked`.
- Full kernel builds were made with this release on the reference laptop: a mainline 7.3.0 default build, a stable 7.2.13 default build (7.2.11 recipe brought to 7.2.13 with both signed updates), and a 7.3.0 real-time configure run that resolves the package name `linux-arxos-rt`. Each passed the configuration gate. The resulting kernels were boot-tested in disposable VMs with kernel taint 0.
- The metadata step was checked on a 7.3.0 build: 6546 file digests identical before and after, and `pacman -Qkk` passes all files.
- A wrong pinned key fingerprint for the stable updates is refused.

## Important limitations

> [!WARNING]
> Do not install a package from this tool on a daily-driver system until it has been inspected and the resulting kernel has been boot-tested in a disposable VM with a bootable fallback available.
