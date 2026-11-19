# Release delivery acceptance

The release workflow calls the CI, desktop, Linux recorder and secure-transfer
workflows from the same Git commit. Packaging begins only after those workflows
and the production agent gates pass. PRs targeting either `main` or `dev` also
run the main CI checks. LoongArch CLI remains an explicitly optional target.

Each packaging job records its commit, version, filenames, sizes and SHA-256
hashes in a provenance file. The final publish job requires macOS universal,
Windows x86_64, Linux x86_64 and Linux aarch64 to succeed. It checks the complete
artifact set and provenance before creating a release. Tagged builds additionally
verify every updater payload with Minisign using the public key embedded in the
installer. A versioned release is uploaded as a draft and made public only after
the uploaded names, sizes and SHA-256 digests match; an already published version
is immutable. A retry verifies its Git tag commit and exact asset contents before
continuing any unfinished channel publication, without changing that version.
Draft lookup falls back to the authenticated paginated release listing because
GitHub's tag lookup only finds published releases; retries reuse existing drafts.
No platform job independently attaches files to a public release.
Both desktop installers and their CLI archives must be present. The two rolling
tags (`dev-latest` and `beta-latest`) move to the verified commit only after
upload succeeds and before publication; versioned tags are never moved. Beta
publication stages both the version and channel assets before making either
draft public. A failure after publishing the version can safely retry the feed.
Publication jobs share a lock per channel. Stable latest and the Beta feed compare
semantic versions, so an older build finishing later cannot move a channel back.
CI uses the commit timestamp for the feed date, keeping publication retries
byte-for-byte reproducible for the same input artifacts.

## Required tag configuration

Set these repository or environment secrets before creating a version tag:

| Secret | Use |
| --- | --- |
| `LUMO_UPDATER_PUBKEY` | Tauri base64 public key; embedded in installers and used to verify payloads |
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri updater signing key; provided only to tagged packaging |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Signing-key password, if the key is encrypted |
| `APPLE_CERTIFICATE` | Base64 Developer ID Application certificate, including its private key, as PKCS#12 |
| `APPLE_CERTIFICATE_PASSWORD` | PKCS#12 password |
| `APPLE_SIGNING_IDENTITY` | Full Developer ID Application signing identity |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | Apple account, app-specific password and team used by Tauri notarization |

The macOS job imports the certificate into a temporary keychain, passes signing
and notarization credentials to the actual build, then checks the app signature,
team ID, stapled ticket and Gatekeeper assessment. It mounts every DMG read-only
and repeats these checks on the shipped app. Its cleanup step restores the
keychain search list and deletes the temporary keychain even after a failure.

`vX.Y.Z` (or `vX.Y.Z-beta.N`) must match both the Cargo workspace and Tauri
versions. The tag gate rejects missing updater keys or mismatched versions.

## Installed update configuration

`build.rs` embeds only public configuration: updater key, stable/beta/rollback
endpoints and channel. A GUI launch without CI environment variables therefore
retains its update configuration. Runtime environment overrides remain available;
an explicitly empty override disables that setting. Endpoints must be HTTPS,
with a host and without credentials in the URL.

Tagged packaging supplies stable and beta endpoints for the current GitHub
repository. A stable tag publishes `latest.json` on its immutable release;
the embedded stable endpoint resolves GitHub's latest stable release. A prerelease
tag also refreshes the `beta-latest` feed, whose payload URLs point to the
immutable versioned release. Rollback is not enabled automatically: an operator
must explicitly configure and validate that channel. These feeds contain signed
payload references; the JSON feed itself is not independently signed.

`main` builds publish an unsigned `dev-latest` prerelease after the same gates.
Development builds embed no update defaults and publish no automatic update feed.
`dev` pushes exercise packaging but do not publish releases.

## Local automated checks

```sh
python3 -m unittest discover -s scripts -p 'test_release_delivery.py' -v
shellcheck scripts/package-ci-release.sh scripts/macos-release-signing.sh
actionlint .github/workflows/*.yml
cargo test --locked -p lumorpa-desktop --lib
```

The release tests cover missing platforms/signatures, changed files, incorrect
commit/version provenance, optional-platform provenance, failed or incomplete
uploads and immutable published versions. With `minisign` installed, a real
temporary signing key proves that the original payload verifies and tampering
fails; CI installs Minisign so this case cannot be skipped there. Publication
tests mock GitHub CLI calls and never publish a release.

## Native and distribution evidence

The [native desktop acceptance](desktop-e2e.md) now exercises canvas insertion,
dragging, undo, port binding, renaming, copy/paste, execution and node results,
plus the companion's text command and cross-window result details using real IPC.
The native runner remains Linux-only. A macOS syntax check or browser probe does
not establish that these native scenarios passed.

Before declaring a release's update path verified, install the preceding signed
version on each supported platform in an isolated account, launch without updater
environment variables, check its reported configuration, then check/download/
install/restart into the new version. Confirm the new version and saved flows;
repeat with a tampered payload and incorrect public key to confirm rejection.
This requires actual signed artifacts and platform runners. The current source
changes and local script tests do not constitute that installation test or a
successful Apple notarization. Preserve CI run URLs, provenance, SHA256SUMS and
the installation test evidence with the release.
