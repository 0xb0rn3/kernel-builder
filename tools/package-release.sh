#!/usr/bin/env bash
# Build portable release bundles plus native Debian/Arch tool packages. Does not install or publish.
set -euo pipefail
HERE=$(cd "$(dirname "$0")/.." && pwd)
VERSION=$(awk -F '"' '/^version = / {print $2; exit}' "$HERE/Cargo.toml")
OUT="${1:?usage: package-release.sh NEW_OUTPUT_DIRECTORY}"
[[ ! -e "$OUT" ]] || { echo 'output exists; choose a fresh directory' >&2; exit 1; }
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
STAGE=$(mktemp -d "$OUT/.stage.XXXXXX")
trap 'rm -rf "$STAGE"' EXIT

# Portable binaries: GNU/glibc for convenience and static musl for broad compatibility.
for ABI in linux linux-musl; do
  if [[ "$ABI" == "linux" ]]; then
    BIN="$HERE/target/release/akt"
  else
    BIN="$HERE/target/x86_64-unknown-linux-musl/release/akt"
  fi
  [[ -x "$BIN" ]] || { echo "missing compiled binary: $BIN" >&2; exit 1; }
  install -Dm755 "$BIN" "$OUT/akt-x86_64-$ABI"
  ROOT="$STAGE/$ABI/kernel-builder"
  mkdir -p "$ROOT"
  install -m755 "$BIN" "$ROOT/akt"
  install -m755 "$HERE/install.sh" "$ROOT/install.sh"
  install -m644 "$HERE/README.md" "$ROOT/README.md"
  cp -R "$HERE/config" "$HERE/patches" "$HERE/ui" "$ROOT/"
  tar --sort=name --owner=0 --group=0 --numeric-owner -czf \
    "$OUT/kernel-builder-v$VERSION-x86_64-$ABI.tar.gz" -C "$STAGE/$ABI" kernel-builder
done

# Distro packages use the static musl binary so the same package does not depend on the
# builder runner's glibc version. The packages install the tool, its GUI and bundled config;
# kernel-compilation dependencies are documented separately and are not silently pulled in.
MUSL_BIN="$HERE/target/x86_64-unknown-linux-musl/release/akt"
[[ -x "$MUSL_BIN" ]] || { echo "missing static musl binary: $MUSL_BIN" >&2; exit 1; }
BUILD_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$HERE" log -1 --format=%ct 2>/dev/null || date +%s)}"

make_data_tree() {
  local root="$1"
  mkdir -p "$root/usr/share/kernel-builder/config" "$root/usr/share/kernel-builder/patches/arxos" \
    "$root/usr/share/kernel-builder/patches/kali" "$root/usr/share/kernel-builder/ui" \
    "$root/usr/share/doc/kernel-builder" "$root/usr/share/applications" "$root/usr/bin"
  install -m755 "$MUSL_BIN" "$root/usr/bin/akt"
  cp -a "$HERE/config/." "$root/usr/share/kernel-builder/config/"
  [[ ! -d "$HERE/patches/arxos" ]] || cp -a "$HERE/patches/arxos/." "$root/usr/share/kernel-builder/patches/arxos/"
  [[ ! -d "$HERE/patches/kali" ]] || cp -a "$HERE/patches/kali/." "$root/usr/share/kernel-builder/patches/kali/"
  install -m644 "$HERE/ui/index.html" "$root/usr/share/kernel-builder/ui/index.html"
  install -m644 "$HERE/README.md" "$root/usr/share/doc/kernel-builder/README.md"
  cat > "$root/usr/share/applications/kernel-builder.desktop" <<'DESK'
[Desktop Entry]
Type=Application
Name=ARXOS Kernel Tuner
GenericName=Kernel Tuner
Comment=Build and manage custom Arch/Debian kernel packages
Exec=akt gui
Icon=applications-system
Terminal=true
Categories=System;Settings;
DESK
}

# Debian .deb package.
DEBROOT="$STAGE/deb"
make_data_tree "$DEBROOT"
mkdir -p "$DEBROOT/DEBIAN"
cat > "$DEBROOT/DEBIAN/control" <<CONTROL
Package: kernel-builder
Version: $VERSION
Section: admin
Priority: optional
Architecture: amd64
Maintainer: ArxOS / Stingray Labs <team@arxos.uk>
Depends: bash, coreutils, git, make, tar, xz-utils, zstd, curl
Description: Machine-aware Arch and Debian kernel builder
 A Rust CLI and local GUI for configuring and building custom Linux kernel
 packages. This package includes akt, GUI assets, config fragments and patches.
 Kernel build toolchains and source dependencies are documented in README.md.
CONTROL
dpkg-deb --build --root-owner-group "$DEBROOT" "$OUT/kernel-builder_${VERSION}_amd64.deb" >/dev/null

# Arch pacman package (.PKGINFO plus a zstd-compressed tar archive).
ARCHROOT="$STAGE/arch"
make_data_tree "$ARCHROOT"
INSTALLED_SIZE=$(du -sb "$ARCHROOT" | awk '{print $1}')
cat > "$ARCHROOT/.PKGINFO" <<PKGINFO
pkgname = kernel-builder
pkgbase = kernel-builder
pkgver = ${VERSION}-1
pkgdesc = Machine-aware Arch and Debian kernel builder
url = https://github.com/0xb0rn3/kernel-builder
builddate = $BUILD_EPOCH
packager = ArxOS / Stingray Labs <team@arxos.uk>
size = $INSTALLED_SIZE
arch = x86_64
license = GPL-3.0-or-later
depend = bash
depend = coreutils
depend = git
depend = curl
depend = make
depend = tar
depend = xz
depend = zstd
PKGINFO
tar --sort=name --owner=0 --group=0 --numeric-owner --zstd -cf \
  "$OUT/kernel-builder-${VERSION}-1-x86_64.pkg.tar.zst" -C "$ARCHROOT" .PKGINFO usr

# Verify package metadata and every published artifact before returning success.
dpkg-deb --info "$OUT/kernel-builder_${VERSION}_amd64.deb" >/dev/null
tar --zstd -xOf "$OUT/kernel-builder-${VERSION}-1-x86_64.pkg.tar.zst" .PKGINFO >/dev/null
(
  cd "$OUT"
  sha256sum \
    akt-x86_64-linux akt-x86_64-linux-musl \
    "kernel-builder-v$VERSION-x86_64-linux.tar.gz" \
    "kernel-builder-v$VERSION-x86_64-linux-musl.tar.gz" \
    "kernel-builder_${VERSION}_amd64.deb" \
    "kernel-builder-${VERSION}-1-x86_64.pkg.tar.zst" > SHA256SUMS
  sha256sum --check SHA256SUMS
)
echo "Release assets ready in $OUT (not uploaded or installed)."
