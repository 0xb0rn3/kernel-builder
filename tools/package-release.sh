#!/usr/bin/env bash
# Package already compiled release binaries. Does not upload or install anything.
set -euo pipefail
HERE=$(cd "$(dirname "$0")/.." && pwd)
VERSION=$(awk -F '"' '/^version = / {print $2; exit}' "$HERE/Cargo.toml")
OUT="${1:?usage: package-release.sh NEW_OUTPUT_DIRECTORY}"
[[ ! -e "$OUT" ]] || { echo 'output exists; use a fresh directory' >&2; exit 1; }
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
STAGE=$(mktemp -d "$OUT/stage.XXXXXX")
for ABI in linux linux-musl; do
  if [[ "$ABI" = linux ]]; then
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
(cd "$OUT" && sha256sum akt-x86_64-linux akt-x86_64-linux-musl \
  "kernel-builder-v$VERSION-x86_64-linux.tar.gz" \
  "kernel-builder-v$VERSION-x86_64-linux-musl.tar.gz" > SHA256SUMS)
echo "Release assets: $OUT (staging retained; no upload)"
