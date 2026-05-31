#!/usr/bin/env bash
set -euo pipefail

# Package the `lumo` CLI into a distributable archive under dist/.
#
# Usage:
#   scripts/package-cli.sh [TARGET]
#
#   TARGET   Optional Rust target triple for cross-compilation, e.g.
#            x86_64-unknown-linux-gnu, aarch64-apple-darwin,
#            x86_64-pc-windows-msvc. When omitted, builds for the host.
#
# Layout (matches the existing dist/ release packages):
#   dist/lumorpa-<version>-<os>-<arch>/
#     bin/lumo            release binary (lumo.exe on Windows)
#     examples/           sample flows + data + skills
#     README.md
#     LICENSE
#   dist/lumorpa-<version>-<os>-<arch>.tar.gz   (.zip for Windows targets)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-}"

cd "$ROOT"

# Version comes from [workspace.package] in the root Cargo.toml.
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
if [[ -z "$VERSION" ]]; then
  echo "error: could not read version from Cargo.toml" >&2
  exit 1
fi

BIN_NAME="lumo"
ARCHIVE="tar.gz"

if [[ -n "$TARGET" ]]; then
  cargo build --release -p lumo-cli --target "$TARGET"
  BIN_DIR="target/$TARGET/release"
  ARCH="${TARGET%%-*}"
  case "$TARGET" in
    *apple-darwin) OS="darwin" ;;
    *linux*)       OS="linux" ;;
    *windows*)     OS="windows"; BIN_NAME="lumo.exe"; ARCHIVE="zip" ;;
    *)             OS="unknown" ;;
  esac
else
  cargo build --release -p lumo-cli
  BIN_DIR="target/release"
  OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
  ARCH="$(uname -m)"
fi

BIN_PATH="$BIN_DIR/$BIN_NAME"
if [[ ! -f "$BIN_PATH" ]]; then
  echo "error: built binary not found at $BIN_PATH" >&2
  exit 1
fi

PKG="lumorpa-${VERSION}-${OS}-${ARCH}"
STAGE="dist/$PKG"

rm -rf "$STAGE"
mkdir -p "$STAGE/bin"
cp "$BIN_PATH" "$STAGE/bin/"
cp -R examples "$STAGE/examples"
# Keep the release examples clean: drop runtime output (gitignored
# examples/out), editor/backup leftovers, and OS cruft that `cp -R` captured.
rm -rf "$STAGE/examples/out"
find "$STAGE/examples" \
  \( -name '*.original' -o -name '*.bak' -o -name '*.tmp' \
     -o -name '*.log' -o -name '.DS_Store' \) -delete
cp README.md LICENSE "$STAGE/"

case "$ARCHIVE" in
  zip)
    if ! command -v zip >/dev/null 2>&1; then
      echo "error: 'zip' command not found (needed for Windows packages)" >&2
      exit 1
    fi
    ( cd dist && rm -f "$PKG.zip" && zip -rq "$PKG.zip" "$PKG" )
    OUT="dist/$PKG.zip"
    ;;
  *)
    tar -C dist -czf "dist/$PKG.tar.gz" "$PKG"
    OUT="dist/$PKG.tar.gz"
    ;;
esac

echo "Packaged $OUT ($(du -h "$OUT" | cut -f1))"
