#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DESKTOP="$ROOT/apps/desktop"
TARGET="${1:-}"
BUNDLES="${2:-}"
OS_NAME="$(uname -s)"
SIMPLE_DMG_STAGE=""
SIMPLE_DMG_RW=""
SIMPLE_DMG_OUT=""

cleanup_simple_dmg() {
  if [[ -n "$SIMPLE_DMG_STAGE" ]]; then
    rm -rf "$SIMPLE_DMG_STAGE"
  fi
  if [[ -n "$SIMPLE_DMG_RW" ]]; then
    rm -f "$SIMPLE_DMG_RW"
  fi
  if [[ -n "$SIMPLE_DMG_OUT" ]]; then
    rm -f "$SIMPLE_DMG_OUT"
  fi
}

trap cleanup_simple_dmg EXIT

case "$OS_NAME" in
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

run_tauri_build() {
  local bundles="$1"

  if [[ -n "$TARGET" ]]; then
    cargo tauri build --target "$TARGET" --bundles "$bundles"
  else
    cargo tauri build --bundles "$bundles"
  fi
}

bundle_contains() {
  local bundles="$1"
  local needle="$2"
  local item

  if [[ "$bundles" == "all" ]]; then
    return 0
  fi

  IFS=',' read -ra bundle_items <<< "$bundles"
  for item in "${bundle_items[@]}"; do
    if [[ "$item" == "$needle" ]]; then
      return 0
    fi
  done

  return 1
}

macos_app_bundles_for_dmg() {
  local bundles="$1"
  local item
  local has_app=0
  local result=()
  local joined=""

  if [[ "$bundles" == "all" || "$bundles" == "dmg" ]]; then
    printf 'app'
    return
  fi

  IFS=',' read -ra bundle_items <<< "$bundles"
  for item in "${bundle_items[@]}"; do
    if [[ -z "$item" || "$item" == "dmg" ]]; then
      continue
    fi
    result+=("$item")
    if [[ "$item" == "app" ]]; then
      has_app=1
    fi
  done

  if [[ "$has_app" -eq 0 ]]; then
    result=("app" "${result[@]}")
  fi

  for item in "${result[@]}"; do
    if [[ -n "$joined" ]]; then
      joined+=","
    fi
    joined+="$item"
  done

  printf '%s' "$joined"
}

read_tauri_json_field() {
  local field="$1"

  if command -v node >/dev/null 2>&1; then
    node -e 'const fs = require("fs"); const config = JSON.parse(fs.readFileSync("src-tauri/tauri.conf.json", "utf8")); process.stdout.write(String(config[process.argv[1]] || ""));' "$field"
  else
    sed -nE 's/^[[:space:]]*"'"$field"'":[[:space:]]*"([^"]+)".*/\1/p' src-tauri/tauri.conf.json | head -n 1
  fi
}

target_release_dir() {
  if [[ -n "$TARGET" ]]; then
    printf '%s/target/%s/release' "$ROOT" "$TARGET"
  else
    printf '%s/target/release' "$ROOT"
  fi
}

macos_bundle_arch() {
  case "$TARGET" in
    *x86_64*)
      printf 'x64'
      ;;
    *aarch64*|*arm64*)
      printf 'aarch64'
      ;;
    *universal*)
      printf 'universal'
      ;;
    *)
      case "$(uname -m)" in
        x86_64)
          printf 'x64'
          ;;
        arm64|aarch64)
          printf 'aarch64'
          ;;
        *)
          uname -m
          ;;
      esac
      ;;
  esac
}

create_simple_macos_dmg() {
  local product_name
  local version
  local arch
  local release_dir
  local app_dir
  local dmg_dir
  local dmg_path
  local tmp_base

  product_name="$(read_tauri_json_field productName)"
  version="$(read_tauri_json_field version)"
  arch="$(macos_bundle_arch)"
  release_dir="$(target_release_dir)"
  app_dir="$release_dir/bundle/macos/${product_name}.app"
  dmg_dir="$release_dir/bundle/dmg"
  dmg_path="$dmg_dir/${product_name}_${version}_${arch}.dmg"

  if [[ -z "$product_name" || -z "$version" ]]; then
    echo "Failed to read productName/version from apps/desktop/src-tauri/tauri.conf.json" >&2
    exit 1
  fi

  if [[ ! -d "$app_dir" ]]; then
    echo "Expected macOS app bundle at $app_dir" >&2
    exit 1
  fi

  mkdir -p "$dmg_dir"

  SIMPLE_DMG_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/lumorpa-dmg-stage.XXXXXX")"
  tmp_base="$(mktemp -u "${TMPDIR:-/tmp}/lumorpa-dmg-rw.XXXXXX")"
  SIMPLE_DMG_RW="${tmp_base}.dmg"
  tmp_base="$(mktemp -u "${TMPDIR:-/tmp}/lumorpa-dmg-out.XXXXXX")"
  SIMPLE_DMG_OUT="${tmp_base}.dmg"

  echo "Bundling ${product_name}_${version}_${arch}.dmg ($dmg_path)"
  echo "Using sandbox-safe simple DMG creation."

  rsync -a --exclude='.DS_Store' "$app_dir" "$SIMPLE_DMG_STAGE/"
  ln -s /Applications "$SIMPLE_DMG_STAGE/Applications"
  hdiutil makehybrid -default-volume-name "$product_name" -hfs -o "$SIMPLE_DMG_RW" "$SIMPLE_DMG_STAGE"
  hdiutil convert "$SIMPLE_DMG_RW" -format UDZO -imagekey zlib-level=9 -ov -o "$SIMPLE_DMG_OUT"
  mv -f "$SIMPLE_DMG_OUT" "$dmg_path"
  SIMPLE_DMG_OUT=""

  echo "Built disk image at: $dmg_path"
}

if [[ "$OS_NAME" == "Darwin" ]] && bundle_contains "$BUNDLES" "dmg"; then
  APP_BUNDLES="$(macos_app_bundles_for_dmg "$BUNDLES")"

  if [[ -n "${CODEX_SANDBOX:-}" || "${LUMORPA_DMG_MODE:-}" == "simple" ]]; then
    run_tauri_build "$APP_BUNDLES"
    create_simple_macos_dmg
    exit 0
  fi

  if run_tauri_build "$BUNDLES"; then
    exit 0
  fi

  status=$?
  if [[ "${LUMORPA_DMG_FALLBACK:-1}" == "0" ]]; then
    exit "$status"
  fi

  echo "Tauri DMG bundling failed; falling back to sandbox-safe simple DMG creation." >&2
  run_tauri_build "$APP_BUNDLES"
  create_simple_macos_dmg
  exit 0
fi

run_tauri_build "$BUNDLES"
