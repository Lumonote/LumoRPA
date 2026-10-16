#!/usr/bin/env bash
set -euo pipefail

# One-shot release packaging: desktop installer + CLI archive from a single
# invocation.
#
# Why this exists
# ---------------
# A release needs a desktop installer *and* a CLI archive per platform, and CI
# previously drove them as two independent steps (`build-desktop.sh` then
# `package-cli.sh`). Both write the same `target/` tree, so cargo's incremental
# cache already spares most of the duplicate work — the recompiled part is
# `lumo-cli` itself plus a second dependency-graph resolve/check pass, not the
# whole tree.
#
# What this buys over calling the two scripts by hand:
#   * one dependency resolve and one version read for the pair, instead of two;
#   * `CARGO_TARGET_DIR` pinned explicitly for both halves, so a caller using a
#     cache mount cannot end up with one half writing to a different tree (which
#     *would* silently force a full rebuild);
#   * both halves reported together, failing closed — a release that produced
#     only its installer, or only its archive, is reported as a failure rather
#     than a partial success.
#
# It is a sequencing wrapper, not a rewrite: both original scripts stay the
# source of truth for their own platform quirks (DMG fallback, updater pubkey
# plumbing, archive layout).
#
# This wrapper keeps both original scripts as the source of truth for their own
# platform quirks (DMG fallback, updater pubkey plumbing, archive layout) and
# only removes the duplicated *scheduling*: it runs the desktop build first,
# then the CLI packaging, sharing one target directory and one version read.
#
# Usage:
#   scripts/package-release.sh [TARGET] [BUNDLES]
#
#   TARGET   Optional Rust target triple, or `universal-apple-darwin`.
#            Empty/omitted builds for the host.
#   BUNDLES  Optional desktop bundle list (app,dmg / nsis,msi / deb,rpm,...).
#            Defaults to the same per-OS default as build-desktop.sh.
#
# Examples:
#   scripts/package-release.sh                                   # host
#   scripts/package-release.sh "" deb,rpm,appimage               # Linux
#   scripts/package-release.sh x86_64-pc-windows-msvc nsis,msi   # Windows
#   scripts/package-release.sh universal-apple-darwin app,dmg    # macOS universal
#
# Set LUMO_SKIP_DESKTOP=1 to produce only the CLI archive, or
# LUMO_SKIP_CLI=1 for only the desktop installer.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-}"
BUNDLES="${2:-}"

cd "$ROOT"

# A single shared target directory. Both child scripts default to `target/`
# already, but pinning it here means a caller exporting CARGO_TARGET_DIR for a
# cache mount gets a consistent answer instead of one script honouring it and
# the other silently writing elsewhere.
if [[ -n "${CARGO_TARGET_DIR:-}" && "${CARGO_TARGET_DIR}" != /* ]]; then
  echo "error: CARGO_TARGET_DIR must be absolute (got '$CARGO_TARGET_DIR')" >&2
  exit 1
fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed -E 's/.*"(.*)".*/\1/')"
if [[ -z "$VERSION" ]]; then
  echo "error: could not read version from Cargo.toml" >&2
  exit 1
fi

echo "== LumoRPA $VERSION release packaging =="
echo "   target:     ${TARGET:-<host>}"
echo "   bundles:    ${BUNDLES:-<per-OS default>}"
echo "   target_dir: $CARGO_TARGET_DIR"
echo

desktop_rc=0
cli_rc=0

if [[ "${LUMO_SKIP_DESKTOP:-0}" != "1" ]]; then
  echo "--> desktop installer"
  set +e
  "$ROOT/scripts/build-desktop.sh" "$TARGET" "$BUNDLES"
  desktop_rc=$?
  set -e
  if [[ "$desktop_rc" -ne 0 ]]; then
    echo "error: desktop packaging failed (exit $desktop_rc)" >&2
  fi
else
  echo "--> desktop installer (skipped: LUMO_SKIP_DESKTOP=1)"
fi
echo

if [[ "${LUMO_SKIP_CLI:-0}" != "1" ]]; then
  echo "--> CLI archive"
  set +e
  "$ROOT/scripts/package-cli.sh" "$TARGET"
  cli_rc=$?
  set -e
  if [[ "$cli_rc" -ne 0 ]]; then
    echo "error: CLI packaging failed (exit $cli_rc)" >&2
  fi
else
  echo "--> CLI archive (skipped: LUMO_SKIP_CLI=1)"
fi
echo

# Report what landed. `dist/` is gitignored, so this is the only record short
# of the CI artifact list.
echo "== artifacts in dist/ =="
if compgen -G "$ROOT/dist/lumorpa-$VERSION-*" > /dev/null; then
  ls -1sh "$ROOT/dist"/lumorpa-"$VERSION"-* | sed 's/^/   /'
else
  echo "   (none)"
fi

# Fail closed: a release that produced only half its artifacts is a broken
# release, not a partial success. Report both statuses so the log says which.
if [[ "$desktop_rc" -ne 0 || "$cli_rc" -ne 0 ]]; then
  echo >&2
  echo "release packaging FAILED (desktop=$desktop_rc cli=$cli_rc)" >&2
  exit 1
fi

echo
echo "release packaging OK"
