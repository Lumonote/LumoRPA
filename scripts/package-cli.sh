#!/usr/bin/env bash
# shellcheck shell=bash
set -euo pipefail

# Package the `lumo` CLI into a distributable archive under dist/.
#
# Usage:
#   scripts/package-cli.sh [TARGET]
#
#   TARGET   Optional Rust target triple for cross-compilation, e.g.
#            x86_64-unknown-linux-gnu, aarch64-apple-darwin,
#            x86_64-pc-windows-msvc. When omitted, builds for the host.
#            The pseudo-target `universal-apple-darwin` builds both macOS
#            arches and `lipo`-merges them into one fat binary (Intel + ARM).
#
# Layout (matches the existing dist/ release packages):
#   dist/lumorpa-<version>-<os>-<arch>/
#     bin/lumo            release binary (lumo.exe on Windows)
#     examples/           sample flows + data + skills
#     README.md
#     LICENSE
#   dist/lumorpa-<version>-<os>-<arch>.tar.gz   (.zip for Windows targets)
#   dist/lumorpa-<version>-<os>-<arch>.tar.gz.sha256   (.zip.sha256 on Windows)
#
# WHY THE ASSERTS BELOW EXIST
# ---------------------------
# An archive that exists but is empty, or that contains a zero-byte binary, is
# a successful-looking artifact that installs nothing. `tar`/`zip` both exit 0
# happily in that case. So before this script claims success it checks the
# archive is non-empty AND that the binary inside it is non-empty, and it
# publishes a .sha256 next to the archive for whoever downloads it.
#
# ENVIRONMENT
#   LUMO_CLI_CARGO_FEATURES=...   Extra cargo flags (e.g. --no-default-features).
#   LUMO_ARTIFACT_MANIFEST=<path> Append kind<TAB>abspath<TAB>bytes<TAB>sha256.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-}"

cd "$ROOT"

# Deterministic collation across macOS bash 3.2 and Git-Bash.
export LC_ALL=C

# Version comes from [workspace.package] in the root Cargo.toml.
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
if [[ -z "$VERSION" ]]; then
  echo "error: could not read version from Cargo.toml" >&2
  exit 1
fi

BIN_NAME="lumo"
ARCHIVE="tar.gz"

# Extra cargo flags for the CLI build, e.g. `--no-default-features`.
#
# Why this exists: lumo-cli's default `hotkey-rdev` feature pulls in rdev,
# which needs X11 headers via pkg-config on Linux. That is correct for a real
# desktop release, but it makes *cross*-compilation impossible without a target
# sysroot - pkg-config refuses to use host .pc files for a foreign target, and
# installing host dev packages does not help. A cross build that only needs the
# CLI (loongarch64 on CI) can pass `--no-default-features`, which selects the
# stub hotkey backend (see the `#[cfg(not(feature = "hotkey-rdev"))]` arm in
# crates/lumo-cli/src/cmd/hotkey.rs). Host builds leave this empty and keep the
# real backend.
CLI_CARGO_FEATURES="${LUMO_CLI_CARGO_FEATURES:-}"

LUMO_ARTIFACT_MANIFEST="${LUMO_ARTIFACT_MANIFEST:-}"
MANIFEST_COMPLETE_SUFFIX=".complete"
MANIFEST_COMPLETE_MARKER="manifest-complete"
MANIFEST_LINES_BEFORE=0

# Same fallback chain as build-desktop.sh: Git-Bash has no sha256sum.
sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    openssl dgst -sha256 "$1" | awk '{print $NF}'
  fi
}

# The Windows release job runs this script under Git Bash, which ships neither
# Info-ZIP's `zip` nor `unzip`. Windows itself ships bsdtar (libarchive) as
# System32\tar.exe, and it can read and write zip archives. Resolve a usable
# bsdtar; an empty result means no zip-capable tool is available.
bsdtar_path() {
  if command -v bsdtar >/dev/null 2>&1; then
    command -v bsdtar
    return 0
  fi
  local winroot
  if command -v cygpath >/dev/null 2>&1; then
    winroot="$(cygpath -u "${WINDIR:-C:\\Windows}" 2>/dev/null || true)"
  fi
  winroot="${winroot:-/c/Windows}"
  if [[ -x "$winroot/System32/tar.exe" ]]; then
    echo "$winroot/System32/tar.exe"
  fi
}

if [[ -n "$LUMO_ARTIFACT_MANIFEST" ]]; then
  mkdir -p "$(dirname "$LUMO_ARTIFACT_MANIFEST")"
  if [[ -f "$LUMO_ARTIFACT_MANIFEST" ]]; then
    MANIFEST_LINES_BEFORE="$(wc -l < "$LUMO_ARTIFACT_MANIFEST" | tr -d '[:space:]')"
  fi
fi

# On failure, rewind the manifest to where this run started. Never truncate to
# empty: the same file may already hold another half's records.
on_exit() {
  local status=$?
  if [[ "$status" -ne 0 && -n "$LUMO_ARTIFACT_MANIFEST" && -e "$LUMO_ARTIFACT_MANIFEST" ]]; then
    local tmp="${LUMO_ARTIFACT_MANIFEST}.rewind.$$"
    if head -n "$MANIFEST_LINES_BEFORE" "$LUMO_ARTIFACT_MANIFEST" > "$tmp" 2>/dev/null; then
      mv -f "$tmp" "$LUMO_ARTIFACT_MANIFEST"
    else
      rm -f "$tmp"
    fi
  fi
  return 0
}
trap on_exit EXIT

if [[ "$TARGET" == "universal-apple-darwin" ]]; then
  # cargo has no native universal target: build each arch, then lipo-merge the
  # two release binaries into a single fat Mach-O (one package, Intel + ARM).
  if ! command -v lipo >/dev/null 2>&1; then
    echo "error: 'lipo' not found (macOS-only; required for universal builds)" >&2
    exit 1
  fi
  cargo build --release -p lumo-cli --target x86_64-apple-darwin $CLI_CARGO_FEATURES
  cargo build --release -p lumo-cli --target aarch64-apple-darwin $CLI_CARGO_FEATURES
  BIN_DIR="target/universal-apple-darwin/release"
  mkdir -p "$BIN_DIR"
  lipo -create \
    "target/x86_64-apple-darwin/release/$BIN_NAME" \
    "target/aarch64-apple-darwin/release/$BIN_NAME" \
    -output "$BIN_DIR/$BIN_NAME"
  OS="darwin"
  ARCH="universal"
elif [[ -n "$TARGET" ]]; then
  cargo build --release -p lumo-cli --target "$TARGET" $CLI_CARGO_FEATURES
  BIN_DIR="target/$TARGET/release"
  ARCH="${TARGET%%-*}"
  case "$TARGET" in
    *apple-darwin) OS="darwin" ;;
    *linux*)       OS="linux" ;;
    *windows*)     OS="windows"; BIN_NAME="lumo.exe"; ARCHIVE="zip" ;;
    *)             OS="unknown" ;;
  esac
else
  cargo build --release -p lumo-cli $CLI_CARGO_FEATURES
  BIN_DIR="target/release"
  OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
  ARCH="$(uname -m)"
fi

BIN_PATH="$BIN_DIR/$BIN_NAME"
if [[ ! -f "$BIN_PATH" ]]; then
  echo "error: built binary not found at $BIN_PATH" >&2
  exit 1
fi

# A zero-byte binary would sail through the archive step and produce a package
# that installs nothing, so reject it while we still know which step made it.
if [[ ! -s "$BIN_PATH" ]]; then
  echo "error: built binary at $BIN_PATH is EMPTY (0 bytes); refusing to package it." >&2
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

if [[ ! -s "$STAGE/bin/$BIN_NAME" ]]; then
  echo "error: staged binary $STAGE/bin/$BIN_NAME is missing or empty; refusing to package it." >&2
  exit 1
fi

case "$ARCHIVE" in
  zip)
    if command -v zip >/dev/null 2>&1; then
      ( cd dist && rm -f "$PKG.zip" && zip -rq "$PKG.zip" "$PKG" )
    else
      # No Info-ZIP (e.g. Windows Git Bash): fall back to the Windows bsdtar,
      # which writes zip with `--format zip`. Failing closed if neither exists.
      BSDTAR="$(bsdtar_path)"
      if [[ -z "$BSDTAR" ]]; then
        echo "error: no zip tool found (need Info-ZIP 'zip' or Windows bsdtar to create '$PKG.zip')" >&2
        exit 1
      fi
      ( cd dist && rm -f "$PKG.zip" && "$BSDTAR" --format zip -cf "$PKG.zip" "$PKG" )
    fi
    OUT="dist/$PKG.zip"
    ;;
  *)
    tar -C dist -czf "dist/$PKG.tar.gz" "$PKG"
    OUT="dist/$PKG.tar.gz"
    ;;
esac

# The archive exists; is it worth shipping?
if [[ ! -s "$OUT" ]]; then
  echo "error: archive $OUT is missing or EMPTY; not a deliverable." >&2
  exit 1
fi

ARCHIVE_BYTES="$(wc -c < "$OUT" | tr -d '[:space:]')"
ARCHIVE_SHA="$(sha256_of "$OUT")"

# Prove the BINARY inside the archive is non-empty. The archive being non-empty
# only means tar/zip wrote headers; a 0-byte payload compresses to a perfectly
# valid archive. Extract just that one member to a scratch file and measure it
# - `tar -xOf` keeps nothing on disk, and `unzip -p` does the same for zip.
BIN_IN_ARCHIVE_BYTES="0"
case "$ARCHIVE" in
  zip)
    if command -v unzip >/dev/null 2>&1; then
      BIN_IN_ARCHIVE_BYTES="$(unzip -p "$OUT" "$PKG/bin/$BIN_NAME" | wc -c | tr -d '[:space:]')"
    else
      # Git Bash has no `unzip` either: bsdtar extracts the member to stdout.
      BSDTAR="$(bsdtar_path)"
      if [[ -z "$BSDTAR" ]]; then
        echo "error: cannot verify '$OUT': need 'unzip' or Windows bsdtar" >&2
        exit 1
      fi
      BIN_IN_ARCHIVE_BYTES="$( "$BSDTAR" -xOf "$OUT" "$PKG/bin/$BIN_NAME" | wc -c | tr -d '[:space:]')"
    fi
    ;;
  *)
    BIN_IN_ARCHIVE_BYTES="$(tar -xOzf "$OUT" "$PKG/bin/$BIN_NAME" | wc -c | tr -d '[:space:]')"
    ;;
esac

if [[ "${BIN_IN_ARCHIVE_BYTES:-0}" -le 0 ]]; then
  echo "error: $OUT contains a 0-byte bin/$BIN_NAME; refusing to call this a package." >&2
  exit 1
fi

# The checksum sidecar sits next to the archive so a downloader can verify the
# file they got is the file we built. `shasum -c` compatible: a bare digest,
# two spaces, then the basename.
CHECKSUM_FILE="$OUT.sha256"
printf '%s  %s\n' "$ARCHIVE_SHA" "$(basename "$OUT")" > "$CHECKSUM_FILE"
if [[ ! -s "$CHECKSUM_FILE" ]]; then
  echo "error: failed to write checksum file $CHECKSUM_FILE" >&2
  exit 1
fi

if [[ -n "$LUMO_ARTIFACT_MANIFEST" ]]; then
  printf '%s\t%s\t%s\t%s\n' "cli-archive" "$ROOT/$OUT" "$ARCHIVE_BYTES" "$ARCHIVE_SHA" >> "$LUMO_ARTIFACT_MANIFEST"
  printf '%s\t%s\t%s\t%s\n' "cli-archive-sha256" "$ROOT/$CHECKSUM_FILE" "$(wc -c < "$CHECKSUM_FILE" | tr -d '[:space:]')" "$(sha256_of "$CHECKSUM_FILE")" >> "$LUMO_ARTIFACT_MANIFEST"

  # The seal names the length of the artifact RECORDS above - everything in the
  # file UP TO BUT NOT INCLUDING the marker line. package-release.sh
  # merge_manifest() verifies exactly that: the first N bytes are the records and
  # whatever follows is one well-formed marker line naming N. The marker cannot
  # state the whole-file length, because the marker is part of the file it would
  # be measuring (its own digit width feeds back into the total).
  #
  # Measured ONCE, here, BEFORE either append, and then reused verbatim for both
  # copies. Measuring twice would hand the sibling file a different number the
  # second time around, because the first printf has already appended the marker
  # line by then - the producer would contradict itself by exactly the marker
  # line's length. Same trap build-desktop.sh documents: `wc -c` must run as its
  # own command before the append, never in the same command as the redirection.
  manifest_bytes="$(wc -c < "$LUMO_ARTIFACT_MANIFEST" | tr -d '[:space:]')"
  printf '%s\t%s\n' "$MANIFEST_COMPLETE_MARKER" "$manifest_bytes" >> "$LUMO_ARTIFACT_MANIFEST"
  printf '%s\t%s\n' "$MANIFEST_COMPLETE_MARKER" "$manifest_bytes" > "${LUMO_ARTIFACT_MANIFEST}${MANIFEST_COMPLETE_SUFFIX}"
fi

echo "Packaged $OUT ($(du -h "$OUT" | cut -f1), $ARCHIVE_BYTES bytes,"
echo "         sha256 $ARCHIVE_SHA)"
echo "Wrote $CHECKSUM_FILE"
echo "Verified bin/$BIN_NAME inside the archive: $BIN_IN_ARCHIVE_BYTES bytes"
