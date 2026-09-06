#!/usr/bin/env bash
# Build + install the ARXOS Kernel Tuner: the `akt` binary plus its bundled tunes, patches, and GUI.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
S=""; [ "$(id -u)" -ne 0 ] && S=sudo

if [ -x "$HERE/akt" ]; then
  BIN="$HERE/akt"; echo ">> using the prebuilt akt binary (release tarball)"
elif [ -x "$HERE/target/release/akt" ]; then
  BIN="$HERE/target/release/akt"; echo ">> using the already-built akt binary"
elif command -v cargo >/dev/null 2>&1; then
  echo ">> building from source (cargo build --release)"; ( cd "$HERE" && cargo build --release ); BIN="$HERE/target/release/akt"
else
  echo "!! no prebuilt akt binary here and no cargo installed — install rust, or download the release tarball."; exit 1
fi

echo ">> installing binary + bundled data"
$S install -Dm755 "$BIN" /usr/local/bin/akt
$S install -d /usr/share/kernel-builder/config /usr/share/kernel-builder/patches/arxos \
              /usr/share/kernel-builder/patches/kali /usr/share/kernel-builder/ui
$S cp -f  "$HERE"/config/*.fragment                 /usr/share/kernel-builder/config/       2>/dev/null || true
$S cp -rf "$HERE"/patches/arxos/.                    /usr/share/kernel-builder/patches/arxos/ 2>/dev/null || true
$S cp -rf "$HERE"/patches/kali/.                     /usr/share/kernel-builder/patches/kali/  2>/dev/null || true
$S install -Dm644 "$HERE/ui/index.html"             /usr/share/kernel-builder/ui/index.html

$S install -Dm644 /dev/stdin /usr/share/applications/kernel-builder.desktop <<'DESK'
[Desktop Entry]
Type=Application
Name=ARXOS Kernel Tuner
GenericName=Kernel Tuner
Comment=Build a tuned, hardened kernel (CachyOS tunes + ARXOS hardening + Kali patches)
Exec=akt gui
Icon=applications-system
Terminal=false
Categories=System;Settings;
DESK

echo ">> installed. Run:  akt gui   (GUI)   |   akt detect / akt plan / akt build   (CLI)"
