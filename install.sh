#!/usr/bin/env bash
# Install prebuilt akt from a release bundle, or build it from source.
# Run as a normal user; sudo is used only for a system-wide installation.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
MODE=""
ASSUME_YES=0

usage() {
  cat <<'USAGE'
Usage: ./install.sh [--system | --standalone] [--yes]

Install the prebuilt release binary when present; otherwise build from source.
If Cargo/Rust is missing, the installer offers to install stable Rust for the
current user via the official rustup installer.

  --system       Install for all users (/usr/local, /usr/share)
  --standalone   Install only for this user (~/.local); interactive default
  --yes          Non-interactive consent to prompts (does not choose scope)
  --help         Show this help

Examples:
  ./install.sh
  ./install.sh --standalone
  ./install.sh --system
  ./install.sh --system --yes
USAGE
}
while (($#)); do
  case "$1" in
    --system) MODE="system" ;;
    --standalone|--user) MODE="standalone" ;;
    --yes|-y) ASSUME_YES=1 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if [[ "$(id -u)" -eq 0 ]]; then
  echo "Run this installer as your normal user, not through sudo." >&2
  echo "It builds with your user Rust toolchain and uses sudo only for system-wide copies." >&2
  exit 2
fi

if [[ -z "$MODE" ]]; then
  if [[ ! -t 0 || "$ASSUME_YES" -eq 1 ]]; then
    echo "Choose an install scope explicitly with --system or --standalone in non-interactive mode." >&2
    exit 2
  fi
  cat <<'CHOICES'
Choose how to install akt:
  1) Standalone, current user only (recommended; no sudo)
     Binary: ~/.local/lib/kernel-builder/akt
     Command: ~/.local/bin/akt
  2) System-wide, all users
     Binary: /usr/local/bin/akt
     Data:   /usr/share/kernel-builder
CHOICES
  read -r -p "Install scope [1/2, default 1]: " answer
  case "${answer:-1}" in
    1|s|S|standalone|user) MODE="standalone" ;;
    2|system|System) MODE="system" ;;
    *) echo "Invalid choice." >&2; exit 2 ;;
  esac
fi

if [[ "$MODE" == "system" ]] && ! command -v sudo >/dev/null 2>&1; then
  echo "System-wide installation needs sudo. Install sudo or choose --standalone." >&2
  exit 2
fi
if [[ ! -f "$HERE/config/arxos.fragment" || ! -f "$HERE/ui/index.html" ]]; then
  echo "Missing config/arxos.fragment or ui/index.html; use a complete checkout/release bundle." >&2
  exit 2
fi

# Make an existing rustup installation visible before checking Cargo.
if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi

BIN=""
if [[ -x "$HERE/akt" ]]; then
  BIN="$HERE/akt"
  echo ">> using prebuilt akt binary; Rust is not required"
else
  if ! command -v cargo >/dev/null 2>&1; then
    install_rust() {
      local rustup_script
      rustup_script=$(mktemp)
      if command -v curl >/dev/null 2>&1; then
        curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 https://sh.rustup.rs -o "$rustup_script"
      elif command -v wget >/dev/null 2>&1; then
        wget -qO "$rustup_script" https://sh.rustup.rs
      elif command -v python3 >/dev/null 2>&1; then
        python3 -c 'import urllib.request; urllib.request.urlretrieve("https://sh.rustup.rs", __import__("sys").argv[1])' "$rustup_script"
      else
        rm -f "$rustup_script"
        echo "Cannot bootstrap Rust automatically: install curl, wget, or python3, then rerun." >&2
        return 1
      fi
      echo ">> installing stable Rust for this user using the official rustup installer"
      sh "$rustup_script" -y --profile minimal --default-toolchain stable
      rm -f "$rustup_script"
      [[ ! -f "$HOME/.cargo/env" ]] || source "$HOME/.cargo/env"
      command -v cargo >/dev/null 2>&1 || {
        echo "Rustup completed but cargo is still not on PATH; open a new shell and rerun." >&2
        return 1
      }
    }
    if [[ "$ASSUME_YES" -ne 1 ]]; then
      read -r -p "Rust/Cargo is missing. Install stable Rust for this user now via rustup? [Y/n]: " answer
      case "${answer:-Y}" in
        y|Y|yes|YES) ;;
        *) echo "No changes made. Install Rust and rerun the installer." >&2; exit 1 ;;
      esac
    fi
    install_rust
  fi

  echo ">> building akt from source (release, locked dependencies)"
  (cd "$HERE" && cargo build --release --locked)
  BIN="$HERE/target/release/akt"
fi
[[ -x "$BIN" ]] || { echo "The akt binary was not produced or is not executable." >&2; exit 1; }

make_desktop_entry() {
  local path="$1" exec_path="$2"
  cat > "$path" <<DESKTOP
[Desktop Entry]
Type=Application
Name=ARXOS Kernel Tuner
GenericName=Kernel Tuner
Comment=Build and manage custom Arch/Debian kernel packages
Exec="$exec_path" gui
Icon=applications-system
Terminal=true
Categories=System;Settings;
DESKTOP
}

copy_data_as_user() {
  local dest="$1"
  mkdir -p "$dest/config" "$dest/patches/arxos" "$dest/patches/kali" "$dest/ui" "$dest/doc"
  cp -a "$HERE/config/." "$dest/config/"
  [[ ! -d "$HERE/patches/arxos" ]] || cp -a "$HERE/patches/arxos/." "$dest/patches/arxos/"
  [[ ! -d "$HERE/patches/kali" ]] || cp -a "$HERE/patches/kali/." "$dest/patches/kali/"
  install -m644 "$HERE/ui/index.html" "$dest/ui/index.html"
  [[ ! -f "$HERE/README.md" ]] || install -m644 "$HERE/README.md" "$dest/doc/README.md"
}

if [[ "$MODE" == "standalone" ]]; then
  DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
  DATA_DIR="$DATA_HOME/kernel-builder"
  LIB_DIR="$HOME/.local/lib/kernel-builder"
  BIN_DIR="$HOME/.local/bin"
  DESKTOP_DIR="$DATA_HOME/applications"
  mkdir -p "$LIB_DIR" "$BIN_DIR"
  copy_data_as_user "$DATA_DIR"
  install -m755 "$BIN" "$LIB_DIR/akt"
  cat > "$BIN_DIR/akt" <<'WRAPPER'
#!/usr/bin/env bash
export AKT_DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/kernel-builder"
exec "$HOME/.local/lib/kernel-builder/akt" "$@"
WRAPPER
  chmod 755 "$BIN_DIR/akt"
  mkdir -p "$DESKTOP_DIR"
  make_desktop_entry "$DESKTOP_DIR/kernel-builder.desktop" "$BIN_DIR/akt"
  echo ">> installed standalone for $USER"
  echo "   Run: $BIN_DIR/akt gui   (or add ~/.local/bin to PATH and run: akt gui)"
  echo "   Data: $DATA_DIR"
else
  sudo -v
  sudo install -Dm755 "$BIN" /usr/local/bin/akt
  sudo install -d /usr/share/kernel-builder/config /usr/share/kernel-builder/patches/arxos \
    /usr/share/kernel-builder/patches/kali /usr/share/kernel-builder/ui /usr/share/doc/kernel-builder
  sudo cp -a "$HERE/config/." /usr/share/kernel-builder/config/
  [[ ! -d "$HERE/patches/arxos" ]] || sudo cp -a "$HERE/patches/arxos/." /usr/share/kernel-builder/patches/arxos/
  [[ ! -d "$HERE/patches/kali" ]] || sudo cp -a "$HERE/patches/kali/." /usr/share/kernel-builder/patches/kali/
  sudo install -m644 "$HERE/ui/index.html" /usr/share/kernel-builder/ui/index.html
  [[ ! -f "$HERE/README.md" ]] || sudo install -m644 "$HERE/README.md" /usr/share/doc/kernel-builder/README.md
  desktop=$(mktemp)
  make_desktop_entry "$desktop" /usr/local/bin/akt
  sudo install -Dm644 "$desktop" /usr/share/applications/kernel-builder.desktop
  rm -f "$desktop"
  echo ">> installed system-wide"
  echo "   Run: akt gui (GUI) | akt system | akt plan | akt build"
fi
