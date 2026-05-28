#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DESKTOP="$ROOT/apps/desktop"
TARGET="${1:-}"
BUNDLES="${2:-}"

case "$(uname -s)" in
  Darwin)
    DEFAULT_BUNDLES="app,dmg"
    ;;
  Linux)
    DEFAULT_BUNDLES="deb,rpm,appimage"
    ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    DEFAULT_BUNDLES="nsis,msi"
    ;;
  *)
    DEFAULT_BUNDLES="all"
    ;;
esac

if [[ -z "$BUNDLES" ]]; then
  BUNDLES="$DEFAULT_BUNDLES"
fi

cd "$DESKTOP"

if [[ -n "$TARGET" ]]; then
  cargo tauri build --target "$TARGET" --bundles "$BUNDLES"
else
  cargo tauri build --bundles "$BUNDLES"
fi

