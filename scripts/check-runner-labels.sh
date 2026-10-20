#!/usr/bin/env bash
# shellcheck shell=bash
#
# check-runner-labels.sh — static guard against retired / floating GitHub Actions
# runner labels.
#
# Usage:
#   scripts/check-runner-labels.sh [--list] [ROOT]
#
#   ROOT     Directory to scan. Defaults to the repo root inferred from this
#            script's own location (so it works from any cwd, including CI).
#   --list   Print the effective scan result (every label found, with the file
#            and line it came from) instead of the terse summary. Use this to
#            paste evidence into a review.
#
# Exit status:
#   0  no retired label found (floating labels may have emitted warnings)
#   1  at least one RETIRED label found — this is what fails CI
#   2  usage error, or the scan roots were missing / unreadable
#
# WHY THIS EXISTS
# ---------------
# GitHub retires runner images on a schedule. When a label is retired it is not
# quietly skipped: for a brownout window the job FAILS at the runner level with
# an error that never mentions the word "retired". It reads exactly like a code
# regression, and the natural reaction is to roll back working code — which
# happened to this repo (a job on macos-14 went green in the same event two
# other macos-14 jobs went red, because the failure is environmental, not code).
#
# The bug also does not have to appear as `runs-on: macos-14`. In this repo it
# arrived through a MATRIX, as:
#     os: [ubuntu-latest, macos-14, windows-latest]
# so a guard that only greps the `runs-on:` line would have missed the very bug
# it was written for. This script scans label *values*, wherever they appear.
#
# NOTE ON FLOATING LABELS: `ubuntu-latest` / `macos-latest` / `windows-latest`
# are deliberately WARN-ONLY, never fatal. A float is a real risk (a silent
# base-image change moves the compiler, glibc and apt package versions under a
# job that asserts exact toolchain behaviour), but the repo uses floats
# legitimately in ci.yml's build-test matrix. Hard-failing on them would make
# this guard fail the repo it exists to protect. Do not "tighten" this to fatal.
#
# Portability: bash 3.2 (macOS) and ubuntu-24.04. No associative arrays, no
# ${var,,}, no GNU-only awk, no yq, no python. Needs only grep/awk/sed/sort.

set -euo pipefail

# ---------------------------------------------------------------------------
# THE LIST TO EDIT WHEN GITHUB RETIRES AN IMAGE NEXT.
# One label per line, every occurrence checked. Keep it one variable.
# ---------------------------------------------------------------------------
RETIRED_LABELS="
macos-14
macos-14-large
macos-14-xlarge
"

# Floating labels: warn only, never fatal. See the note above before changing
# this into a failure.
FLOATING_LABELS="
ubuntu-latest
macos-latest
windows-latest
"

usage() {
  sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
}

root=""
list_mode=0

while [ $# -gt 0 ]; do
  case "$1" in
    --list) list_mode=1 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "check-runner-labels: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [ -n "$root" ]; then
        echo "check-runner-labels: unexpected extra argument: $1" >&2
        exit 2
      fi
      root="$1"
      ;;
  esac
  shift
done

if [ -z "$root" ]; then
  # Resolve the directory holding this script, then its parent => repo root.
  script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
  root=$(CDPATH= cd -- "$script_dir/.." && pwd)
fi

if [ ! -d "$root" ]; then
  echo "check-runner-labels: scan root is not a directory: $root" >&2
  exit 2
fi

# ---------------------------------------------------------------------------
# Collect the files to scan: .github/workflows/*.yml and .github/actions/**/action.yml
# ---------------------------------------------------------------------------
files=""
add_files() {
  # $1 = directory, $2 = -maxdepth value, $3 = filename glob
  [ -d "$1" ] || return 0
  found=$(find "$1" -maxdepth "$2" -type f -name "$3" 2>/dev/null | sort || true)
  if [ -n "$found" ]; then
    if [ -n "$files" ]; then
      files="$files
$found"
    else
      files="$found"
    fi
  fi
}

add_files "$root/.github/workflows" 1 '*.yml'
add_files "$root/.github/workflows" 1 '*.yaml'
add_files "$root/.github/actions" 3 'action.yml'
add_files "$root/.github/actions" 3 'action.yaml'

# Fall back to scanning the whole .github tree if neither canned pattern matched
# anything — a repo that renames these directories should still be guarded, not
# silently pass because the scanner found no files at all.
if [ -z "$files" ] && [ -d "$root/.github" ]; then
  files=$(find "$root/.github" -type f \( -name '*.yml' -o -name '*.yaml' \) 2>/dev/null | sort || true)
fi

if [ -z "$files" ]; then
  echo "check-runner-labels: found no workflow/action YAML under $root/.github" >&2
  echo "check-runner-labels: refusing to report success on an empty scan" >&2
  exit 2
fi

label_is_in() {
  # $1 = label to test, $2 = newline-separated list
  # Prints "yes"/"no" so callers stay bash-3.2 safe (no ${var,,}).
  printf '%s\n' "$2" | awk -v want="$1" '
    {
      line=$0
      gsub(/^[ \t]+|[ \t]+$/, "", line)
      if (line == "") next
      if (line == want) { found=1; exit }
    }
    END { if (found) print "yes"; else print "no" }
  '
}

# ---------------------------------------------------------------------------
# Scan.
#
# A line can carry label values in two shapes:
#   1. `runs-on: ubuntu-24.04`                    (scalar)
#   2. `os: [ubuntu-latest, macos-14, windows-latest]`  (flow sequence)
#   3. block sequence:
#        os:
#          - ubuntu-latest
#          - macos-14
# We take every token that "looks like a runner label" (word + dashes/digits/
# dots, optionally quoted) from any line, then classify it against the lists.
# Anything not in a list is ignored, so `ubuntu-22.04` and `ubuntu-22.04-arm`
# pass even though they are pinned to a specific (and supported) image.
# ---------------------------------------------------------------------------
tmp_hits=$(mktemp -t runner-labels.XXXXXX)
tmp_all=$(mktemp -t runner-labels-all.XXXXXX)
# shellcheck disable=SC2064
trap "rm -f '$tmp_hits' '$tmp_all'" EXIT INT TERM

retired_count=0
floating_count=0
scanned_count=0

while IFS= read -r file; do
  [ -n "$file" ] || continue
  [ -f "$file" ] || continue
  scanned_count=$((scanned_count + 1))

  # rel_path: strip the scan root for readable output.
  case "$file" in
    "$root"/*) rel_path=${file#"$root"/} ;;
    *) rel_path="$file" ;;
  esac

  # Extract candidate label tokens. awk (POSIX) keeps this identical on macOS.
  candidates=$(awk '
    {
      line=$0
      # Drop trailing comments so "runs-on: x  # pinned" does not yield "pinned".
      sub(/[ \t]+#.*$/, "", line)
      n = split(line, tok, /[^A-Za-z0-9_.-]+/)
      for (i = 1; i <= n; i++) {
        t = tok[i]
        # A runner label: starts with a letter, contains a '-', no '$'/'{'/'%'.
        if (t !~ /^[A-Za-z]/) continue
        if (t !~ /-/) continue
        if (t ~ /[^A-Za-z0-9_.-]/) continue
        print t
      }
    }
  ' "$file" 2>/dev/null || true)

  [ -n "$candidates" ] || continue

  # $candidates is newline-separated; iterate it with a here-doc so bash 3.2 is happy.
  while IFS= read -r token; do
    [ -n "$token" ] || continue
    case "$token" in
      *-latest|macos-*|ubuntu-*|windows-*) ;;
      *) continue ;;
    esac

    if [ "$(label_is_in "$token" "$RETIRED_LABELS")" = "yes" ]; then
      printf '%s\n' "$rel_path:$token" >> "$tmp_hits"
      printf 'RETIRED  %s  %s\n' "$rel_path" "$token" >> "$tmp_all"
      retired_count=$((retired_count + 1))
    elif [ "$(label_is_in "$token" "$FLOATING_LABELS")" = "yes" ]; then
      printf 'FLOATING %s  %s\n' "$rel_path" "$token" >> "$tmp_all"
      floating_count=$((floating_count + 1))
    fi
  done <<EOF
$candidates
EOF
done <<EOF
$files
EOF

# ---------------------------------------------------------------------------
# Report.
# ---------------------------------------------------------------------------
retired_hits=$(sort -u "$tmp_hits" 2>/dev/null || true)
all_lines=$(sort -u "$tmp_all" 2>/dev/null || true)

if [ "$list_mode" -eq 1 ]; then
  echo "check-runner-labels: scanned $scanned_count file(s) under $root"
  echo "check-runner-labels: --list (one line per distinct file:label hit)"
  if [ -n "$all_lines" ]; then
    printf '%s\n' "$all_lines"
  else
    echo "  (no classified labels found)"
  fi
  echo "check-runner-labels: totals: retired=$retired_count floating=$floating_count"
fi

if [ -n "$retired_hits" ]; then
  echo "" >&2
  echo "FAIL: retired GitHub Actions runner label(s) found." >&2
  echo "GitHub retires runner images on a schedule; a retired label does not" >&2
  echo "skip — the job FAILS at the runner level with an error that never says" >&2
  echo "'retired', so it reads like a code regression and invites a rollback of" >&2
  echo "working code. Move the job to a supported pinned label." >&2
  echo "" >&2
  printf '%s\n' "$retired_hits" | sed 's/^/  retired  /' >&2
  echo "" >&2
  echo "To fix: edit the offending file(s) above, then update RETIRED_LABELS in" >&2
  echo "  scripts/check-runner-labels.sh" >&2
  echo "only when you also intend to retire the check for that label." >&2
  exit 1
fi

if [ "$floating_count" -gt 0 ]; then
  echo ""
  echo "warning: floating runner label(s) in use (NOT a failure):"
  printf '%s\n' "$all_lines" | grep '^FLOATING' | sed 's/^/  /' || true
  echo "  A floating label can silently move the base image under a job that"
  echo "  asserts exact toolchain behaviour. Prefer a pinned label there."
fi

echo "OK: no retired runner labels in $scanned_count scanned file(s)."
exit 0
