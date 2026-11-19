#!/usr/bin/env bash
# Package a CI build with public updater defaults embedded in the binary.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ "${GITHUB_REF:-}" == refs/tags/v* ]]; then
  : "${GITHUB_REPOSITORY:?repository is required for update endpoints}"
  : "${LUMO_UPDATER_PUBKEY:?tagged builds require a public update key}"
  : "${TAURI_SIGNING_PRIVATE_KEY:?tagged builds require a private signing key}"
  export LUMO_UPDATE_ENDPOINT="${LUMO_UPDATE_ENDPOINT:-https://github.com/${GITHUB_REPOSITORY}/releases/latest/download/latest.json}"
  export LUMO_UPDATE_BETA_ENDPOINT="${LUMO_UPDATE_BETA_ENDPOINT:-https://github.com/${GITHUB_REPOSITORY}/releases/download/beta-latest/latest.json}"
  if [[ "${GITHUB_REF_NAME:-}" == *-* ]]; then
    export LUMO_UPDATE_CHANNEL=beta
  else
    export LUMO_UPDATE_CHANNEL=stable
  fi
else
  # Development snapshots deliberately carry no update/signing configuration.
  unset LUMO_UPDATER_PUBKEY TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD
  unset LUMO_UPDATE_ENDPOINT LUMO_UPDATE_BETA_ENDPOINT LUMO_UPDATE_ROLLBACK_ENDPOINT LUMO_UPDATE_CHANNEL
fi
exec "$ROOT/scripts/package-release.sh" "$@"
