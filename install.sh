#!/usr/bin/env bash
# Install prebuilt akt when present; otherwise bootstrap a user-local Rust toolchain and build.
set -Eeuo pipefail

HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
MODE=""
YES=0
ALLOW_RUST_INSTALL=1

usage() {
  cat <<'USAGE'
Usage: ./install.sh [--standalone | --user | --system] [--yes] [--no-rust-install]

  --standalone       Keep akt and its data in this directory; no sudo or PATH changes.
  --user             Install for this user under ~/.local; no system directories are changed.
  --system           Install to /usr/local/bin and /usr/share/kernel-builder (uses sudo if needed).
  --yes              Accept prompts, including user-local Rust installation when required.
  --no-rust-install  Do not install Rust automatically; fail if no usable binary/cargo exists.
  --help             Show this help.

Release bundles already contain a compiled binary, so Rust is not needed to install those bundles.
With no install mode, an interactive menu asks where to install.
USAGE
}

while (($#)); do
  case "$1" in
    --standalone) MODE=standalone ;;
    --user) MODE=user ;;
    --system|--system-wide) MODE=system ;;
    --yes|-y) YES=1 ;;
    --no-rust-install) ALLOW_RUST_INSTALL=0 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if [[ -z "$MODE" ]]; then
  if [[ ! -t 0 ]]; then
    echo "Choose a mode for non-interactive use: --standalone, --user, or --system." >&2
    exit 2
  fi
  cat <<'MENU'
ARXOS Kernel Tuner installation
  1) Standalone in this directory (no sudo, no PATH/profile changes)
  2) Current user (~/.local/bin and ~/.local/share; no sudo)
  3) System-wide (/usr/local/bin and /usr/share; sudo may be requested)
MENU
  read -r -p "Select [1-3, default 1]: " choice
  case "${choice:-1}" in
    1) MODE=standalone ;;
    2) MODE=user ;;
    3) MODE=system ;;
    *) echo "Invalid selection." >&2; exit 2 ;;
  esac
fi

case "$MODE" in
  standalone)
    BIN_DEST="$HERE/akt"
    DATA_DIR="$HERE"
    DESKTOP_DIR=""
    ;;
  user)
    [[ -n "${HOME:-}" && "$HOME" != "/" ]] || { echo "HOME is not set to a usable user directory." >&2; exit 1; }
    BIN_DEST="$HOME/.local/bin/akt"
    DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
    [[ "$DATA_HOME" == /* ]] || DATA_HOME="$HOME/.local/share"
    DATA_DIR="$DATA_HOME/kernel-builder"
    DESKTOP_DIR="$DATA_HOME/applications"
    ;;
  system)
    BIN_DEST="/usr/local/bin/akt"
    DATA_DIR="/usr/share/kernel-builder"
    DESKTOP_DIR="/usr/share/applications"
    if [[ "$(id -u)" -ne 0 ]] && ! command -v sudo >/dev/null 2>&1; then
      echo "System-wide install requires sudo, or run this script as root." >&2
      exit 1
    fi
    ;;
esac

if [[ ! -f "$HERE/config/arxos.fragment" || ! -f "$HERE/ui/index.html" ]]; then
  echo "Missing config/ or ui/. Use the complete release bundle or a source checkout." >&2
  exit 1
fi

ask_yes() {
  local prompt="$1" answer=""
  [[ "$YES" -eq 1 ]] && return 0
  [[ -t 0 ]] || return 1
  read -r -p "$prompt [Y/n] " answer
  case "${answer,,}" in
    n|no) return 1 ;;
    *) return 0 ;;
  esac
}

ensure_rust() {
  if command -v cargo >/dev/null 2>&1; then
    return 0
  fi
  if [[ "$ALLOW_RUST_INSTALL" -ne 1 ]]; then
    echo "No prebuilt akt binary or cargo found, and --no-rust-install was set." >&2
    return 1
  fi
  if [[ "$(id -u)" -eq 0 ]]; then
    echo "Run install.sh as a normal user; it builds Rust and akt without root, then requests sudo only for system-wide file copies." >&2
    return 1
  fi
  if ! ask_yes "No prebuilt binary or cargo found. Install Rust stable under ~/.local/share/akt/toolchain?"; then
    echo "Install Rust stable, then rerun ./install.sh. Nothing was installed." >&2
    return 1
  fi
  command -v curl >/dev/null 2>&1 || {
    echo "curl is required to fetch the official rustup installer. Install curl and retry." >&2
    return 1
  }

  local user_data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
  [[ "$user_data_home" == /* ]] || user_data_home="$HOME/.local/share"
  local toolroot="$user_data_home/akt/toolchain"
  export CARGO_HOME="${AKT_CARGO_HOME:-$toolroot/cargo}"
  export RUSTUP_HOME="${AKT_RUSTUP_HOME:-$toolroot/rustup}"
  export PATH="$CARGO_HOME/bin:$PATH"
  local rustup_script
  rustup_script=$(mktemp)
  trap 'rm -f "$rustup_script"' RETURN
  echo ">> fetching the official rustup installer over HTTPS"
  curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
    https://sh.rustup.rs -o "$rustup_script"
  echo ">> installing minimal Rust stable in $toolroot (no shell profile changes)"
  sh "$rustup_script" -y --profile minimal --default-toolchain stable --no-modify-path
  rm -f "$rustup_script"
  trap - RETURN
  command -v cargo >/dev/null 2>&1
}

if [[ -x "$HERE/akt" ]]; then
  BIN="$HERE/akt"
  echo ">> using the prebuilt akt binary"
elif [[ -x "$HERE/target/release/akt" ]]; then
  BIN="$HERE/target/release/akt"
  echo ">> using the already-built release binary"
else
  if [[ "$(id -u)" -eq 0 ]]; then
    echo "Do not run a source build as root. Run ./install.sh as your normal user; it will request sudo only if you choose system-wide installation." >&2
    exit 1
  fi
  ensure_rust || exit 1
  echo ">> building akt from source (cargo build --release --locked; no root build)"
  (cd "$HERE" && cargo build --release --locked)
  BIN="$HERE/target/release/akt"
fi

[[ -x "$BIN" ]] || { echo "Build completed without an executable binary at $BIN." >&2; exit 1; }

as_target() {
  if [[ "$MODE" == system && "$(id -u)" -ne 0 ]]; then
    sudo "$@"
  else
    "$@"
  fi
}

if [[ "$MODE" == standalone ]]; then
  if [[ "$BIN" != "$BIN_DEST" ]]; then
    install -m755 "$BIN" "$BIN_DEST"
  fi
  echo
  echo "Standalone install ready in: $HERE"
  echo "Run: $HERE/akt gui"
  echo "Data and configuration remain beside the binary; no PATH or desktop entry was changed."
  exit 0
fi

if [[ "$MODE" == system && "$(id -u)" -ne 0 ]]; then
  echo ">> requesting permission for system-wide installation"
  sudo -v
fi

echo ">> installing binary to $BIN_DEST"
as_target install -Dm755 "$BIN" "$BIN_DEST"
echo ">> installing configuration, patches, and GUI to $DATA_DIR"
as_target install -d "$DATA_DIR/config" "$DATA_DIR/patches/arxos" "$DATA_DIR/patches/kali" "$DATA_DIR/ui"
as_target cp -a "$HERE/config/." "$DATA_DIR/config/"
as_target cp -a "$HERE/patches/arxos/." "$DATA_DIR/patches/arxos/"
as_target cp -a "$HERE/patches/kali/." "$DATA_DIR/patches/kali/"
as_target install -Dm644 "$HERE/ui/index.html" "$DATA_DIR/ui/index.html"

if [[ -n "$DESKTOP_DIR" ]]; then
  desktop=$(mktemp)
  cat >"$desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=ARXOS Kernel Tuner
GenericName=Kernel Tuner
Comment=Build a tuned kernel for Arch or Debian-family Linux
Exec="$BIN_DEST" gui
Icon=applications-system
Terminal=false
Categories=System;Settings;
DESKTOP
  as_target install -Dm644 "$desktop" "$DESKTOP_DIR/kernel-builder.desktop"
  rm -f "$desktop"
fi

echo
echo ">> installation complete"
if [[ "$MODE" == user ]]; then
  case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *) echo "Add this to your shell PATH if needed: export PATH=\"$HOME/.local/bin:\$PATH\"" ;;
  esac
  echo "Run: $BIN_DEST gui"
else
  echo "Run: akt gui   (GUI)   |   akt detect / akt plan / akt build   (CLI)"
fi
echo "Note: install the kernel build dependencies for your host before running akt build."
