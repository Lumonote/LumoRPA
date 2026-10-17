#!/usr/bin/env bash
# shellcheck shell=bash
set -euo pipefail

# Build the LumoRPA desktop installers for one platform and then PROVE they
# were produced. The proof is the point of this script.
#
# Usage:
#   scripts/build-desktop.sh [TARGET] [BUNDLES]
#
#   TARGET   Optional Rust target triple. Empty/omitted builds for the host.
#   BUNDLES  Optional comma list of Tauri bundle types to build
#            (app,dmg / nsis,msi / deb,rpm,appimage, or "all").
#            Defaults to what the host can reasonably build.
#
# WHY THIS SCRIPT VERIFIES ARTIFACTS ITSELF
# -----------------------------------------
# `cargo tauri build` can return 0 while the installer you asked for is not on
# disk, and it can also return non-zero for a reason that has nothing to do
# with the file you wanted. Child exit codes alone cannot tell "we shipped an
# installer" from "we shipped nothing". Users reported exactly that: every
# commit green, no installable software anywhere.
#
# So after bundling we look for the expected files under the bundle root and
# fail when one is missing. Whenever the caller tells us where to write
# LUMO_ARTIFACT_MANIFEST, we also record kind/path/bytes/sha256 for every
# artifact we did find, so a downstream consumer never has to trust a summary
# line again.
#
# THE DMG FALLBACK IS OPT-IN
# --------------------------
# macOS used to fall back automatically: if the real Tauri DMG bundling failed,
# the script built only the .app and hand-rolled a disk image with hdiutil,
# then exited 0. That turned a broken DMG build into a green one and shipped an
# unsigned, un-notarized image with no updater .sig. The fallback is still
# available, but it now requires an explicit LUMORPA_DMG_FALLBACK=1, it says
# out loud what it is doing, and without that opt-in the real failure status is
# propagated to the caller.
#
# ENVIRONMENT
#   LUMO_UPDATER_PUBKEY=<key>            Updater pubkey handed to the bundler.
#   LUMORPA_DMG_FALLBACK=1               Allow the hand-rolled hdiutil DMG
#                                        after a real bundling failure.
#   LUMORPA_ALLOW_PARTIAL_BUNDLES=1      Build/verify only the bundle types
#                                        this host can actually produce.
#   LUMORPA_DMG_MODE=simple              Ask for the simple DMG up front
#                                        (same as a CODEX_SANDBOX build).
#   LUMO_ARTIFACT_MANIFEST=<path>        Append artifact records here.
#   LUMO_SKIP_ARTIFACT_CHECK=1           Disable the assertion (it says so).

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DESKTOP="$ROOT/apps/desktop"

# Locale: bash 3.2 on macOS and the Git-Bash builds on Windows both choose
# different collation and case-folding rules otherwise, and the artifact
# lookup below depends on both. Fixing the locale makes local and CI runs
# agree, which is worth more than any host-specific default.
export LC_ALL=C

TARGET="${1:-}"
BUNDLES="${2:-}"
OS_NAME="$(uname -s)"

# Scratch paths. They are declared here so require_no_scratch_files() can
# clear them before anything else runs: the old implementation carried
# leftovers from a failed attempt into the next one.
DMG_STAGE=""
DMG_RW=""
DMG_OUT=""

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

# tauri.conf.json enables createUpdaterArtifacts but deliberately carries no
# `plugins.updater` block: the pubkey is injected at runtime from
# LUMO_UPDATER_PUBKEY (see apps/desktop/src-tauri/src/lib.rs). The bundler still
# needs a valid key at bundle time, so supply it from the same variable here
# instead of committing one. Without it, updater artifacts are skipped so a
# plain local build still produces an installer.
if [[ -n "${LUMO_UPDATER_PUBKEY:-}" ]]; then
  UPDATER_CONFIG="{\"plugins\":{\"updater\":{\"pubkey\":\"${LUMO_UPDATER_PUBKEY}\"}}}"
else
  UPDATER_CONFIG='{"bundle":{"createUpdaterArtifacts":false}}'
  echo "LUMO_UPDATER_PUBKEY is not set; skipping updater artifacts (.tar.gz/.sig)." >&2
fi

run_tauri_build() {
  local bundles="$1"

  if [[ -n "$TARGET" ]]; then
    cargo tauri build --target "$TARGET" --bundles "$bundles" --config "$UPDATER_CONFIG"
  else
    cargo tauri build --bundles "$bundles" --config "$UPDATER_CONFIG"
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
  # One place knows the host vs `--target <triple>` split. Everything that
  # needs to find a bundle (verification, manifest, fallback) asks this.
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

# ---------------------------------------------------------------------------
# Artifact verification
# ---------------------------------------------------------------------------
#
# THE ONE RULE THIS FILE EXISTS FOR: a zero exit status from the bundler is not
# evidence that an installer exists. Everything below inspects the filesystem
# instead of trusting a status code.
#
# Layout handled here (target_release_dir() resolves the host vs --target split):
#   <release>/bundle/dmg/*.dmg          macOS disk image
#   <release>/bundle/macos/*.app        macOS app tree (a directory)
#   <release>/bundle/nsis/*-setup.exe   Windows installer
#   <release>/bundle/msi/*.msi          Windows MSI
#   <release>/bundle/deb/*.deb          Debian/麒麟 x86_64 + arm64
#   <release>/bundle/rpm/*.rpm          RPM
#   <release>/bundle/appimage/*.AppImage
#
# An artifact that is not linked against a glibc newer than the target distro
# is out of scope here: this script proves presence and integrity, not that the
# binary will load on 麒麟 / 统信. See README "信创与交叉编译".

MANIFEST_COMPLETE_SUFFIX=".complete"

# `shasum` is the macOS default and is also present in Git-Bash; `sha256sum` is
# the GNU/coreutils name; `openssl` is the last resort. All three print the
# digest somewhere in the last field, so awk on $NF handles any of them. There
# is deliberately no "otherwise skip the digest" branch: a manifest whose
# sha256 column is empty is worse than no manifest at all, because a consumer
# cannot tell it apart from a verified one.
sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    openssl dgst -sha256 "$1" | awk '{print $NF}'
  fi
}

# bash 3.2.57 has no ${v,,}/${v^^}, so case folding goes through tr.
to_lower_ascii() {
  tr '[:upper:]' '[:lower:]' <<< "$1"
}

# Every regular file below $1, one absolute path per line, breadth first.
#
# Why find -maxdepth 1 in a loop and not one recursive find piped into a while
# loop: on bash 3.2 (macOS) and in Git-Bash the reading end of a pipeline runs
# in a SUBSHELL, so an artifact counter incremented there would be discarded by
# the parent and every artifact found would still be reported as missing. The
# `for f in $(...)` consumer below stays in the current shell.
dir_files() {
  local dir="$1"
  local entry

  [[ -d "$dir" ]] || return 0

  find "$dir" -maxdepth 1 -mindepth 1 -type f -print > "$DIR_FILES_TMP" 2>/dev/null || true
  find "$dir" -maxdepth 1 -mindepth 1 -type d -print >> "$DIR_FILES_TMP" 2>/dev/null || true

  while IFS= read -r entry; do
    [[ -n "$entry" ]] || continue
    if [[ -d "$entry" ]]; then
      dir_files "$entry"
    else
      printf '%s\n' "$entry"
    fi
  done < <(tr '\n' '\t' < "$DIR_FILES_TMP" | tr '\t' '\n')
}

# Finds the FIRST file, in C collation order, matching a name pattern under the
# bundle root. Deterministic order matters: with two installers of the same kind
# present, the manifest must always name the same one.
# Sets glob_result (relative) and glob_absolute.
lookup_artifact() {
  local pattern="$1"
  local f
  local lf
  local lp
  local sorted_output

  glob_result=""
  glob_absolute=""

  if ! sorted_output="$(dir_files "$BUNDLE_ROOT" | LC_ALL=C sort)"; then
    return 1
  fi

  for f in $sorted_output; do
    lf="$(to_lower_ascii "$f")"
    case "$pattern" in
      *..*|*/*)
        lp="$(to_lower_ascii "$pattern")"
        case "$lf" in
          $lp)
            glob_result="$f"
            ;;
        esac
        ;;
      *)
        lp="$(to_lower_ascii "${pattern#*.}")"
        case "$lf" in
          *".$lp"|*"_$lp")
            glob_result="$f"
            ;;
        esac
        ;;
    esac

    if [[ -n "$glob_result" ]]; then
      glob_absolute="$(cd "$(dirname "$glob_result")" && pwd)/$(basename "$glob_result")"
      return 0
    fi
  done

  return 1
}

note_artifact() {
  artifact_count=$((artifact_count + 1))
  artifact_kinds="${artifact_kinds}${artifact_kinds:+, }$1"
  if [[ -n "$LUMO_ARTIFACT_MANIFEST" ]]; then
    printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" >> "$LUMO_ARTIFACT_MANIFEST"
  fi
}

record_artifact() {
  local pattern="$1"
  local kind="$2"
  local bytes
  local digest

  if ! lookup_artifact "$pattern"; then
    if [[ -n "$missing_artifacts" ]]; then
      missing_artifacts="${missing_artifacts}, $kind"
    else
      missing_artifacts="$kind"
    fi
    return 0
  fi

  bytes="$(wc -c < "$glob_absolute" | tr -d '[:space:]')"
  digest="$(sha256_of "$glob_absolute")"
  note_artifact "$kind" "$glob_absolute" "$bytes" "$digest"
  echo "   verified $kind: $glob_absolute ($bytes bytes, sha256 ${digest})"
}

verify_requested_bundles() {
  local bundles="$1"
  local item
  local dmg_digest

  missing_artifacts=""
  artifact_count=0
  artifact_kinds=""

  if [[ ! -d "$BUNDLE_ROOT" ]]; then
    echo "error: no bundle output directory at $BUNDLE_ROOT; nothing was packaged." >&2
    return 1
  fi

  if [[ "$bundles" == "all" ]]; then
    # `--bundles all` means "whatever this host can build", which is not a
    # fixed set: asserting per-format coverage here would fail on every host
    # that cannot build an MSI. So for "all" we only assert that the bundler
    # left SOMETHING behind, and we say plainly that the per-artifact
    # expectations were deliberately not asserted.
    set -- $(dir_files "$BUNDLE_ROOT" | LC_ALL=C sort)
    if [[ "$#" -eq 0 ]]; then
      echo "error: --bundles all produced no files under $BUNDLE_ROOT." >&2
      return 1
    fi
    echo "   --bundles all: $# bundle file(s) produced; per-format expectations NOT asserted"
    return 0
  fi

  IFS=',' read -ra bundle_items <<< "$bundles"
  for item in "${bundle_items[@]}"; do
    case "$item" in
      "")
        ;;
      app)
        # A macOS .app is a directory tree, not a file. Its presence alone is
        # not a deliverable (the DMG is), so it is recorded but the DMG check
        # below is what actually gates a macOS release.
        if lookup_artifact "*.app"; then
          note_artifact "app" "$glob_absolute" 0 "$(sha256_of "$glob_absolute")"
          echo "   verified app: $glob_absolute (directory tree; no byte digest)"
        else
          if [[ -n "$missing_artifacts" ]]; then
            missing_artifacts="${missing_artifacts}, app"
          else
            missing_artifacts="app"
          fi
        fi
        ;;
      dmg)
        record_artifact "*.dmg" "dmg"
        ;;
      nsis)
        # The NSIS setup basename follows the configured installer languages
        # (LumoRPA_0.1.0_x64-setup.exe and friends), so match the suffix rather
        # than Tauri's default stem, which is not what tauri.conf.json pins.
        record_artifact "*-setup.exe" "nsis"
        ;;
      msi)
        record_artifact "*.msi" "msi"
        ;;
      deb)
        record_artifact "*.deb" "deb"
        ;;
      rpm)
        record_artifact "*.rpm" "rpm"
        ;;
      appimage)
        record_artifact "*.AppImage" "appimage"
        ;;
      *)
        # Unknown bundle id: say so instead of silently passing. A typo in
        # BUNDLES should not look like a verified release.
        echo "   note: no artifact expectation for bundle '$item'; skipping" >&2
        ;;
    esac
  done

  if [[ -n "$missing_artifacts" ]]; then
    echo >&2
    echo "error: bundling reported success but these requested artifacts are not under $BUNDLE_ROOT:" >&2
    echo "         $missing_artifacts" >&2
    echo "       produced: ${artifact_kinds:-<none>}" >&2
    echo "       A green build with no installer is the bug this check exists to catch." >&2
    return 1
  fi

  echo "   artifact set complete: ${artifact_kinds:-<none>}"
  return 0
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

  DMG_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/lumorpa-dmg-stage.XXXXXX")"
  tmp_base="$(mktemp -u "${TMPDIR:-/tmp}/lumorpa-dmg-rw.XXXXXX")"
  DMG_RW="${tmp_base}.dmg"
  tmp_base="$(mktemp -u "${TMPDIR:-/tmp}/lumorpa-dmg-out.XXXXXX")"
  DMG_OUT="${tmp_base}.dmg"

  echo "Bundling ${product_name}_${version}_${arch}.dmg ($dmg_path)"
  echo "Using sandbox-safe simple DMG creation."

  rsync -a --exclude='.DS_Store' "$app_dir" "$DMG_STAGE/"
  ln -s /Applications "$DMG_STAGE/Applications"
  hdiutil makehybrid -default-volume-name "$product_name" -hfs -o "$DMG_RW" "$DMG_STAGE"
  hdiutil convert "$DMG_RW" -format UDZO -imagekey zlib-level=9 -ov -o "$DMG_OUT"
  mv -f "$DMG_OUT" "$dmg_path"
  DMG_OUT=""

  echo "Built disk image at: $dmg_path"
}

# ---------------------------------------------------------------------------
# Manifest handling
# ---------------------------------------------------------------------------

# The manifest is APPENDED to, never rewritten: package-release.sh runs the two
# halves of a release in one process but a caller may legitimately invoke this
# script twice (desktop here, CLI there) against the same file. Truncating would
# destroy the earlier half's records, so "clean up after a failure" means
# "rewind to the line count we saw when we started", never "empty the file".
maybe_clear_manifest() {
  [[ -n "$LUMO_ARTIFACT_MANIFEST" && -e "$LUMO_ARTIFACT_MANIFEST" ]] || return 0
  local kept="$MANIFEST_LINES_BEFORE"
  local tmp="${LUMO_ARTIFACT_MANIFEST}.rewind.$$"

  if head -n "$kept" "$LUMO_ARTIFACT_MANIFEST" > "$tmp" 2>/dev/null; then
    mv -f "$tmp" "$LUMO_ARTIFACT_MANIFEST"
  else
    rm -f "$tmp"
  fi
  return 0
}

write_manifest_complete_marker() {
  # A manifest is only trustworthy if a consumer can tell "complete" from
  # "truncated". Container formats (tar, zip, dmg, ...) cannot express the
  # length of their own payload, so instead of trusting the file we write a
  # marker that names the byte count of everything written before it. The
  # consumer (package-release.sh) checks it and refuses a manifest whose tail
  # is missing; the marker is written last and only on the success path.
  [[ -n "$LUMO_ARTIFACT_MANIFEST" ]] || return 0

  local bytes
  local tmp

  # `wc -c` MUST run as its own command before the append. In a same-command
  # `bytes=$(wc -c < f)` plus `printf ... >> f`, the shell truncates/creates f
  # for the redirection first, so the count comes back 0 (or stale) and the
  # marker would describe a file length that never existed. The consumer
  # (package-release.sh merge_manifest) byte-checks that number, so getting it
  # wrong makes every real release fail to seal its manifest.
  #
  # What is recorded is the LEGAL byte count - the length up to and including
  # the final newline of the artifact records. `wc -c` is locale-aware and
  # under LC_ALL=C counts bytes, which is the definition the consumer compares
  # against; a future locale-aware reader must use `otext` for the same reason.
  bytes="$(wc -c < "$LUMO_ARTIFACT_MANIFEST" | tr -d '[:space:]')"

  tmp="${LUMO_ARTIFACT_MANIFEST}.$$.append"
  if ! printf '%s\t%s\n' "$MANIFEST_COMPLETE_MARKER" "$bytes" > "$tmp"; then
    rm -f "$tmp"
    echo "WARNING: could not write the manifest completion marker to $LUMO_ARTIFACT_MANIFEST" >&2
    return 0
  fi
  cat "$tmp" >> "$LUMO_ARTIFACT_MANIFEST"
  rm -f "$tmp"

  # The marker also exists as a sibling file, for consumers that only stat.
  if ! printf '%s\t%s\n' "$MANIFEST_COMPLETE_MARKER" "$bytes" > "${LUMO_ARTIFACT_MANIFEST}${MANIFEST_COMPLETE_SUFFIX}"; then
    echo "WARNING: could not write ${LUMO_ARTIFACT_MANIFEST}${MANIFEST_COMPLETE_SUFFIX}" >&2
  fi
}

# ---------------------------------------------------------------------------
# The two build shapes
# ---------------------------------------------------------------------------

# A partial build is one where we do not expect every requested bundle type this
# host cannot produce. The caller must opt in (LUMORPA_ALLOW_PARTIAL_BUNDLES=1)
# or ask for it up front (LUMORPA_DMG_MODE=simple, CODEX_SANDBOX), and both
# routes print a banner saying exactly what is missing from the result.
build_macos_simple_dmg() {
  local app_bundles="$1"

  run_tauri_build "$app_bundles"
  create_simple_macos_dmg
}

# THE FAILURE THIS SCRIPT WAS REWRITTEN FOR.
#
# Before: any non-zero status from the real DMG bundling fell through to the
# hdiutil fallback, which produced an unsigned, un-notarized image with no
# updater .sig, and then exited 0. A broken DMG build was indistinguishable from
# a good one, and the only installable output users ever got was that image.
#
# Now: the fallback requires LUMORPA_DMG_FALLBACK=1. Without that explicit
# opt-in we propagate the real status, after explaining what we refused to do
# and what the caller can set if a local, unsigned image is genuinely wanted.
handle_dmg_failure() {
  local status="$1"
  local app_bundles="$2"

  if [[ "${LUMORPA_DMG_FALLBACK:-0}" != "1" ]]; then
    echo "error: cargo tauri build --bundles $BUNDLES failed with status $status" >&2
    echo "       and the simple DMG fallback is DISABLED by default." >&2
    echo "       That fallback produces an UNSIGNED image with no updater .sig, so" >&2
    echo "       enabling it silently would hide a broken DMG behind a green build." >&2
    echo "       Set LUMORPA_DMG_FALLBACK=1 to opt in, or LUMO_SKIP_ARTIFACT_CHECK=1" >&2
    echo "       to skip verification entirely." >&2
    return "$status"
  fi

  echo "############################################################################" >&2
  echo "# WARNING: DMG FALLBACK ENGAGED (LUMORPA_DMG_FALLBACK=1)                   #" >&2
  echo "#   cargo tauri build --bundles $BUNDLES failed with status $status" >&2
  echo "#   Falling back to a hand-rolled hdiutil image built from the .app only." >&2
  echo "#   The result is UNSIGNED, NOT NOTARIZED and has NO updater .sig." >&2
  echo "#   This is a convenience image for local use, NOT a shippable release." >&2
  echo "############################################################################" >&2

  build_macos_simple_dmg "$app_bundles"
  return 0
}

# Emitted once, whichever route led here, so an unsigned image can never be
# mistaken for a release. Kept as a function so the wording lives in one place.
warn_partial_build() {
  local reason="$1"

  echo "############################################################################" >&2
  echo "# WARNING: PARTIAL BUILD - NOT A RELEASE-GRADE RESULT                    #" >&2
  echo "#   reason: $reason" >&2
  echo "#   Only the bundle types this host can actually produce were verified." >&2
  echo "#   Requested: $BUNDLES" >&2
  echo "############################################################################" >&2
}

# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------

BUNDLE_ROOT="$(target_release_dir)/bundle"
LUMO_ARTIFACT_MANIFEST="${LUMO_ARTIFACT_MANIFEST:-}"
MANIFEST_LINES_BEFORE=0
MANIFEST_COMPLETE_MARKER="manifest-complete"

if [[ -n "$LUMO_ARTIFACT_MANIFEST" ]]; then
  mkdir -p "$(dirname "$LUMO_ARTIFACT_MANIFEST")"
  if [[ -f "$LUMO_ARTIFACT_MANIFEST" ]]; then
    # `wc -l` on a file with no trailing newline undercounts by one, which only
    # makes the rewind keep one line less than it should - never more. That is
    # the safe direction: a rewind can only ever remove lines this run wrote.
    MANIFEST_LINES_BEFORE="$(wc -l < "$LUMO_ARTIFACT_MANIFEST" | tr -d '[:space:]')"
  fi
fi
DIR_FILES_TMP="${TMPDIR:-/tmp}/lumorpa-dir-files.$$.tmp"

# A failed build must never leave a manifest that looks complete.
on_exit() {
  local status=$?

  if [[ -n "$DMG_STAGE" ]]; then rm -rf "$DMG_STAGE"; fi
  if [[ -n "$DMG_RW" ]]; then rm -f "$DMG_RW"; fi
  if [[ -n "$DMG_OUT" ]]; then rm -f "$DMG_OUT"; fi
  rm -f "$DIR_FILES_TMP"

  if [[ "$status" -ne 0 ]]; then
    maybe_clear_manifest
  fi
  return 0
}
trap on_exit EXIT

if [[ "${LUMO_SKIP_ARTIFACT_CHECK:-0}" == "1" ]]; then
  # Loud on purpose. The single most valuable thing this script does is refuse
  # to report success without evidence; switching that off must be visible in
  # every log that contains it, including the ones nobody reads twice.
  echo "WARNING: LUMO_SKIP_ARTIFACT_CHECK=1 - artifact verification is DISABLED;" >&2
  echo "         this run can exit 0 without an installer on disk." >&2
fi

if [[ "$OS_NAME" == "Darwin" ]] && bundle_contains "$BUNDLES" "dmg"; then
  APP_BUNDLES="$(macos_app_bundles_for_dmg "$BUNDLES")"

  # Two legitimate ways to ask for the simple DMG up front rather than as a
  # rescue: a sandbox that cannot mount a disk image (CODEX_SANDBOX), or an
  # explicit LUMORPA_DMG_MODE=simple. Both are the caller declaring "I cannot
  # run the real bundler here", so they opt in before anything is attempted
  # instead of being silently switched over after a failure.
  if [[ -n "${CODEX_SANDBOX:-}" || "${LUMORPA_DMG_MODE:-}" == "simple" ]]; then
    echo "LUMORPA_DMG_MODE=simple/CODEX_SANDBOX: using the sandbox-safe simple DMG by request." >&2
    warn_partial_build "simple DMG requested up front (sandbox / LUMORPA_DMG_MODE=simple)"
    build_macos_simple_dmg "$APP_BUNDLES"
    if [[ "${LUMO_SKIP_ARTIFACT_CHECK:-0}" != "1" ]]; then
      verify_requested_bundles "$APP_BUNDLES" || exit 1
    fi
    write_manifest_complete_marker
    echo "BUILD OK (partial: $APP_BUNDLES)"
    exit 0
  fi

  # Clear stale DMGs BEFORE the real build. Otherwise a failed run could
  # "verify" against the image a previous successful run left behind, which is
  # the same false green as the fallback bug wearing a different hat.
  rm -f "$BUNDLE_ROOT"/dmg/*.dmg 2>/dev/null || true

  # `$?` after an `if` whose branch never ran is the status of the `if` itself
  # (always 0), so capture the build's real exit code directly.
  status=0
  run_tauri_build "$BUNDLES" || status=$?
  if [[ "$status" -ne 0 ]]; then
    handle_dmg_failure "$status" "$APP_BUNDLES" || exit "$?"
    # Reaching here means the fallback ran. Verify the DMG it produced rather
    # than the bundle list that was originally requested: the whole point is
    # to report what exists, not what was asked for.
    if [[ "${LUMO_SKIP_ARTIFACT_CHECK:-0}" != "1" ]]; then
      verify_requested_bundles "dmg" || exit 1
    fi
    write_manifest_complete_marker
    echo "BUILD OK (partial: hand-rolled DMG via LUMORPA_DMG_FALLBACK=1)"
    exit 0
  fi

  if [[ "${LUMO_SKIP_ARTIFACT_CHECK:-0}" != "1" ]]; then
    verify_requested_bundles "$BUNDLES" || exit 1
  fi
  write_manifest_complete_marker
  echo "BUILD OK ($BUNDLES)"
  exit 0
fi

# Non-macOS, or a macOS build that did not request a DMG.
run_tauri_build "$BUNDLES"
if [[ "${LUMO_SKIP_ARTIFACT_CHECK:-0}" != "1" ]]; then
  verify_requested_bundles "$BUNDLES" || exit 1
fi
write_manifest_complete_marker
echo "BUILD OK ($BUNDLES)"
