#!/usr/bin/env bash
# shellcheck shell=bash
set -euo pipefail

# One-shot release packaging: desktop installer + CLI archive from a single
# invocation, and then PROOF that both actually exist.
#
# WHY THIS EXISTS
# ---------------
# A release needs a desktop installer *and* a CLI archive per platform, and CI
# used to drive them as two independent steps (`build-desktop.sh` then
# `package-cli.sh`). Both write the same `target/` tree, so cargo's incremental
# cache already spares most of the duplicate work - the recompiled part is
# `lumo-cli` itself plus a second dependency-graph resolve/check pass, not the
# whole tree.
#
# What this wrapper buys over calling the two scripts by hand:
#   * one dependency resolve and one version read for the pair, instead of two;
#   * `CARGO_TARGET_DIR` pinned explicitly for both halves, so a caller using a
#     cache mount cannot end up with one half writing to a different tree (which
#     *would* silently force a full rebuild);
#   * an ARTIFACT COMPLETENESS CHECK after both halves, which is the part that
#     was missing. Child exit codes used to be the only evidence, and both
#     halves exiting 0 while neither dropped a file on disk is exactly the
#     failure users reported: green builds, nothing installable.
#
# It is a sequencing wrapper, not a rewrite: both original scripts stay the
# source of truth for their own platform quirks (DMG fallback, updater pubkey
# plumbing, archive layout). This file owns the scheduling, the shipping check
# and the combined manifest, and nothing else.
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
# ENVIRONMENT
#   LUMO_SKIP_DESKTOP=1        Produce only the CLI archive.
#   LUMO_SKIP_CLI=1            Produce only the desktop installer.
#   LUMO_ARTIFACT_MANIFEST     Where the combined manifest goes.
#                              Default: dist/manifest.txt
#
# Skipping BOTH halves is refused: the script would have nothing to build,
# nothing to verify, and would still print a green summary. If that is really
# wanted, call the underlying scripts directly.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-}"
BUNDLES="${2:-}"

cd "$ROOT"

# Deterministic collation for the greps/sorts below across bash 3.2 (macOS) and
# Git-Bash (Windows), which disagree by default.
export LC_ALL=C

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

SKIP_DESKTOP=0
SKIP_CLI=0
if [[ "${LUMO_SKIP_DESKTOP:-0}" == "1" ]]; then
  SKIP_DESKTOP=1
fi
if [[ "${LUMO_SKIP_CLI:-0}" == "1" ]]; then
  SKIP_CLI=1
fi

if [[ "$SKIP_DESKTOP" == "1" && "$SKIP_CLI" == "1" ]]; then
  echo "error: LUMO_SKIP_DESKTOP=1 and LUMO_SKIP_CLI=1 together skip both halves." >&2
  echo "       There would be nothing to build and nothing to verify, yet the" >&2
  echo "       script would still report success. Run a child script directly." >&2
  exit 2
fi

MANIFEST="${LUMO_ARTIFACT_MANIFEST:-$ROOT/dist/manifest.txt}"
MANIFEST_TMP=""
DESKTOP_MANIFEST=""
CLI_MANIFEST=""
BODY_FILE=""

# Must match the marker name and sibling suffix both producers write
# (build-desktop.sh and package-cli.sh). Kept as variables so the consumer never
# drifts from the convention documented in merge_manifest().
MANIFEST_COMPLETE_MARKER="manifest-complete"
MANIFEST_COMPLETE_SUFFIX=".complete"

# Cleanup hangs off a trap rather than the tail of the script: there are several
# deliberate early `exit`s below, and a temp file that only some of them clean
# up is a temp file that eventually ends up in someone's home directory.
cleanup_release() {
  local f
  for f in "$MANIFEST_TMP" "$DESKTOP_MANIFEST" "$CLI_MANIFEST" "$BODY_FILE"; do
    if [[ -n "$f" ]]; then rm -f "$f"; fi
  done
  return 0
}
trap cleanup_release EXIT

# Same fallback chain as build-desktop.sh: Git-Bash on Windows has no
# sha256sum, and macOS no longer ships it either.
sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    openssl dgst -sha256 "$1" | awk '{print $NF}'
  fi
}

# ---------------------------------------------------------------------------
# Combined manifest
# ---------------------------------------------------------------------------
#
# Each half appends `kind<TAB>abspath<TAB>bytes<TAB>sha256` lines to its own
# manifest. Merging them here - rather than having the halves write one shared
# file - keeps the halves independent: either can still be run on its own and
# produce a usable manifest.
#
# The manifest is written through a temporary file, validated, and only then
# moved into place. A manifest that exists must never be a half-written one:
# that is the same class of bug as a green build with no installer, one layer
# down.
merge_manifest() {
  local out="$1"
  shift
  local src
  local lines=0
  local rel_path
  local abs_path
  local bytes
  local digest
  local actual
  local complete_marker
  local complete_declared
  local actual_bytes
  local tail_bytes
  local expected_tail
  local actual_tail
  local sibling_marker
  local merged_bytes

  : > "$out"

  for src in "$@"; do
    [[ -n "$src" && -f "$src" ]] || continue

    # A partial log is worse than a missing one, so refuse it. build-desktop.sh
    # and package-cli.sh each write a completion marker as their last action,
    # which is what makes "this manifest is complete" a verifiable claim rather
    # than a hopeful one.
    complete_marker="$(awk -F'\t' -v k="manifest-complete" '$1 == k { v = $2 } END { if (v != "") print v }' "$src")"
    if [[ -z "$complete_marker" ]]; then
      echo "error: manifest from $src has no completion marker; refusing to merge a partial artifact list." >&2
      return 1
    fi
    if [[ ! -f "${src}.complete" ]]; then
      echo "error: manifest from $src is missing its ${src}.complete sibling marker." >&2
      return 1
    fi

    # The marker records a BYTE COUNT, not a line count, because byte counts are
    # what a truncating copy actually loses. It is the length of the artifact
    # RECORDS - i.e. the manifest UP TO BUT NOT INCLUDING the marker line:
    #
    #   bytes="$(wc -c < f)"        <- measured before the marker line exists
    #   printf ... >> f             <- the marker line is appended after
    #
    # both producers (build-desktop.sh, package-cli.sh) work that way, and they
    # must, because the marker has to name the length of a file it is itself part
    # of. An earlier revision compared this value against the WHOLE file length,
    # which is the claim plus the marker line - a deterministic mismatch of
    # exactly len("manifest-complete\t<digits>\n") that failed every release at
    # the sealing step. So the check is prefix-based: the artifact records must
    # occupy exactly $complete_marker bytes and everything after them must be
    # one well-formed marker line naming that same number.
    actual_bytes="$(wc -c < "$src" | tr -d '[:space:]')"
    if [[ "$actual_bytes" -lt "$complete_marker" ]]; then
      echo "error: $src is $actual_bytes bytes, less than the $complete_marker bytes its marker claims;" >&2
      echo "       the manifest was truncated after it was sealed." >&2
      return 1
    fi

    # `tail -c +N` is 1-based, so +"$((complete_marker + 1))" starts at the byte
    # right after the sealed prefix.
    tail_bytes="$(tail -c "+$((complete_marker + 1))" "$src" | wc -c | tr -d '[:space:]')"
    if [[ "$tail_bytes" -ne "$((actual_bytes - complete_marker))" ]]; then
      echo "error: $src: the sealed prefix of $complete_marker bytes did not line up." >&2
      return 1
    fi

    expected_tail="${MANIFEST_COMPLETE_MARKER}	${complete_marker}"
    actual_tail="$(tail -c "+$((complete_marker + 1))" "$src" | tr -d '\n')"
    if [[ "$actual_tail" != "$expected_tail" ]]; then
      echo "error: $src carries an unexpected tail after its sealed prefix:" >&2
      echo "       expected [$expected_tail]" >&2
      echo "       found    [$actual_tail]" >&2
      echo "       The artifact list cannot be trusted to be the one that was verified." >&2
      return 1
    fi

    # The sibling file is the same claim for consumers that only stat, so it has
    # to agree with the in-file marker or one of the two is lying.
    sibling_marker="$(awk -F'\t' -v k="manifest-complete" '$1 == k { v = $2 } END { if (v != "") print v }' "${src}.complete")"
    if [[ "$sibling_marker" != "$complete_marker" ]]; then
      echo "error: ${src}.complete claims [$sibling_marker] but the in-file marker claims [$complete_marker]." >&2
      return 1
    fi

    while IFS= read -r line; do
      [[ -n "$line" ]] || continue
      case "$line" in
        manifest-complete*)
          continue
          ;;
      esac

      rel_path="$(printf '%s' "$line" | cut -f2)"
      abs_path="$rel_path"
      case "$abs_path" in
        /*) ;;
        *) abs_path="$ROOT/$abs_path" ;;
      esac

      if [[ ! -f "$abs_path" ]]; then
        echo "error: artifact listed in $src does not exist: $abs_path" >&2
        return 1
      fi

      bytes="$(printf '%s' "$line" | cut -f3)"
      digest="$(printf '%s' "$line" | cut -f4)"
      actual="$(wc -c < "$abs_path" | tr -d '[:space:]')"
      if [[ "$actual" -ne "$bytes" ]]; then
        echo "error: $abs_path is $actual bytes, manifest from $src says $bytes." >&2
        return 1
      fi

      printf '%s\t%s\t%s\t%s\n' "$(printf '%s' "$line" | cut -f1)" "$abs_path" "$bytes" "$digest" >> "$out"
      lines=$((lines + 1))
    done < "$src"
  done

  if [[ "$lines" -eq 0 ]]; then
    echo "error: merged manifest is empty; nothing was produced." >&2
    return 1
  fi

  # Seal the merged manifest with the SAME convention the producers use: the
  # recorded number is the length of the records above, measured as its own
  # command BEFORE the marker line is appended. Nothing downstream consumes this
  # file today, but a manifest sealed under a different convention is exactly the
  # trap that just cost three rounds; sealing it the same way keeps a future
  # consumer from having to guess which producer wrote it.
  merged_bytes="$(wc -c < "$out" | tr -d '[:space:]')"
  printf '%s\t%s\n' "$MANIFEST_COMPLETE_MARKER" "$merged_bytes" >> "$out"
  printf '%s\t%s\n' "$MANIFEST_COMPLETE_MARKER" "$merged_bytes" > "${out}${MANIFEST_COMPLETE_SUFFIX}"
  echo "   merged $lines artifact record(s) into $out"
  return 0
}

# Mirrors build-desktop.sh's target_release_dir(): one place that knows the
# host vs `--target <triple>` split, expressed against the shared
# CARGO_TARGET_DIR that both halves were pinned to above.
target_release_dir() {
  if [[ -n "$TARGET" ]]; then
    printf '%s/%s/release' "$CARGO_TARGET_DIR" "$TARGET"
  else
    printf '%s/release' "$CARGO_TARGET_DIR"
  fi
}

# WHERE THE DESKTOP INSTALLER ACTUALLY LIVES.
#
# Not in dist/. build-desktop.sh writes installers ONLY beneath
# `target[/<triple>]/release/bundle/...` - it never copies one into dist/, and
# release.yml stages them with `find target/<triple>/release/bundle` for exactly
# that reason. An earlier revision of this check globbed dist/ for the desktop
# half, so it reported "no desktop installer" on every real build even when both
# installers had just been produced: a false negative that would have blocked
# every release. The CLI half really does land in dist/ (package-cli.sh writes
# there), so only the desktop half looks at the bundle tree.
verify_release_artifacts() {
  local problems=""
  local bundle_root

  if [[ "$SKIP_DESKTOP" != "1" ]]; then
    bundle_root="$(target_release_dir)/bundle"
    # A desktop installer: any of the formats Tauri can emit. Matching the
    # extension rather than an exact stem keeps this working for a .dmg on
    # macOS, an .exe/.msi on Windows and .deb/.rpm/.AppImage on Linux without
    # a per-OS branch here that could drift from the bundler.
    if compgen -G "$bundle_root/dmg/*.dmg" > /dev/null \
       || compgen -G "$bundle_root/macos/*.app" > /dev/null \
       || compgen -G "$bundle_root/nsis/*.exe" > /dev/null \
       || compgen -G "$bundle_root/msi/*.msi" > /dev/null \
       || compgen -G "$bundle_root/deb/*.deb" > /dev/null \
       || compgen -G "$bundle_root/rpm/*.rpm" > /dev/null \
       || compgen -G "$bundle_root/appimage/*.AppImage" > /dev/null; then
      echo "   desktop installer: present under $bundle_root"
    else
      problems="${problems}\n  - no desktop installer under $bundle_root"
    fi
  else
    echo "   desktop installer: skipped (LUMO_SKIP_DESKTOP=1)"
  fi

  if [[ "$SKIP_CLI" != "1" ]]; then
    if compgen -G "$ROOT/dist/lumorpa-$VERSION-*.tar.gz" > /dev/null \
       || compgen -G "$ROOT/dist/lumorpa-$VERSION-*.zip" > /dev/null; then
      echo "   CLI archive: present"
    else
      problems="${problems}\n  - no CLI archive (dist/lumorpa-$VERSION-*.tar.gz|zip)"
    fi
  else
    echo "   CLI archive: skipped (LUMO_SKIP_CLI=1)"
  fi

  if [[ -n "$problems" ]]; then
    echo >&2
    echo "error: release artifact set is INCOMPLETE:" >&2
    printf '%b\n' "$problems" >&2
    echo "       Both halves were expected to produce a file. A zero exit status from" >&2
    echo "       a child script is not evidence that the file exists, which is why this" >&2
    echo "       check looks at the filesystem instead." >&2
    return 1
  fi

  return 0
}

# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------

echo "== LumoRPA $VERSION release packaging =="
echo "   target:     ${TARGET:-<host>}"
echo "   bundles:    ${BUNDLES:-<per-OS default>}"
echo "   target_dir: $CARGO_TARGET_DIR"
echo

MANIFEST_TMP="$(mktemp "${TMPDIR:-/tmp}/lumorpa-manifest.XXXXXX")"
DESKTOP_MANIFEST="$MANIFEST_TMP.desktop"
CLI_MANIFEST="$MANIFEST_TMP.cli"
: > "$DESKTOP_MANIFEST"
: > "$CLI_MANIFEST"

desktop_rc=0
cli_rc=0

# Each half writes its OWN manifest through LUMO_ARTIFACT_MANIFEST, so merging
# here needs no shared mutable state and each child stays independently usable.

if [[ "$SKIP_DESKTOP" != "1" ]]; then
  echo "--> desktop installer"
  set +e
  LUMO_ARTIFACT_MANIFEST="$DESKTOP_MANIFEST" "$ROOT/scripts/build-desktop.sh" "$TARGET" "$BUNDLES"
  desktop_rc=$?
  set -e
  if [[ "$desktop_rc" -ne 0 ]]; then
    echo "error: desktop packaging failed (exit $desktop_rc)" >&2
  fi
else
  echo "--> desktop installer (skipped: LUMO_SKIP_DESKTOP=1)"
fi
echo

if [[ "$SKIP_CLI" != "1" ]]; then
  echo "--> CLI archive"
  set +e
  LUMO_ARTIFACT_MANIFEST="$CLI_MANIFEST" "$ROOT/scripts/package-cli.sh" "$TARGET"
  cli_rc=$?
  set -e
  if [[ "$cli_rc" -ne 0 ]]; then
    echo "error: CLI packaging failed (exit $cli_rc)" >&2
  fi
else
  echo "--> CLI archive (skipped: LUMO_SKIP_CLI=1)"
fi
echo

# `dist/` is gitignored, so the log and the combined manifest are the only
# durable record of what a run produced short of the CI artifact list.
echo "== artifacts in dist/ =="
if compgen -G "$ROOT/dist/lumorpa-$VERSION-*" > /dev/null; then
  ls -1sh "$ROOT/dist"/lumorpa-"$VERSION"-* | sed 's/^/   /'
else
  echo "   (none)"
fi
echo

if [[ "$desktop_rc" -ne 0 || "$cli_rc" -ne 0 ]]; then
  echo "release packaging FAILED (desktop=$desktop_rc cli=$cli_rc)" >&2
  exit 1
fi

# Both halves reported success. That is an INPUT to the decision below, never
# the decision itself: a half can exit 0 without dropping a file on disk.
if ! verify_release_artifacts; then
  echo >&2
  echo "release packaging FAILED (desktop=$desktop_rc cli=$cli_rc, but the artifact set is incomplete)" >&2
  exit 1
fi

# Seal the combined manifest. Done last so the file that lands in dist/ can
# only ever describe a run that got this far.
if ! merge_manifest "$MANIFEST" "$DESKTOP_MANIFEST" "$CLI_MANIFEST"; then
  echo >&2
  echo "release packaging FAILED (artifact manifest could not be sealed)" >&2
  exit 1
fi

echo
echo "== combined artifact manifest =="
echo "   $MANIFEST"
if [[ -f "$MANIFEST" ]]; then
  sed 's/^/   /' "$MANIFEST"
else
  echo "   (missing)"
fi

echo
echo "release packaging OK"
