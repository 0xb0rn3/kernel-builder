#!/usr/bin/env bash
# Build + install the ARXOS Kernel Tuner: the `akt` binary plus its bundled tunes, patches, and GUI.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
S=""; [ "$(id -u)" -ne 0 ] && S=sudo

echo ">> building (cargo build --release)"
( cd "$HERE" && cargo build --release )

echo ">> installing binary + bundled data"
$S install -Dm755 "$HERE/target/release/akt" /usr/local/bin/akt
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
