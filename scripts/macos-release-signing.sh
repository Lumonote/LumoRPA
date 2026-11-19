#!/usr/bin/env bash
# Import into an isolated temporary keychain and verify the actual app after build.
set -euo pipefail
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
KEYCHAIN="$RUNNER_TEMP/lumo-release-signing.keychain-db"
CERTIFICATE="$RUNNER_TEMP/lumo-release-signing.p12"
case "${1:-}" in
  import)
    : "${APPLE_CERTIFICATE:?missing APPLE_CERTIFICATE}"
    : "${APPLE_CERTIFICATE_PASSWORD:?missing APPLE_CERTIFICATE_PASSWORD}"
    : "${APPLE_SIGNING_IDENTITY:?missing APPLE_SIGNING_IDENTITY}"
    : "${APPLE_ID:?missing APPLE_ID}"
    : "${APPLE_PASSWORD:?missing APPLE_PASSWORD}"
    : "${APPLE_TEAM_ID:?missing APPLE_TEAM_ID}"
    umask 077
    python3 - "$CERTIFICATE" <<'PY'
import base64, os, pathlib, sys
pathlib.Path(sys.argv[1]).write_bytes(base64.b64decode(os.environ['APPLE_CERTIFICATE'], validate=True))
PY
    PASSWORD="$(openssl rand -hex 32)"
    security create-keychain -p "$PASSWORD" "$KEYCHAIN"
    security set-keychain-settings -lut 21600 "$KEYCHAIN"
    security unlock-keychain -p "$PASSWORD" "$KEYCHAIN"
    security import "$CERTIFICATE" -P "$APPLE_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12 -k "$KEYCHAIN"
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$PASSWORD" "$KEYCHAIN" >/dev/null
    python3 - "$KEYCHAIN" "$RUNNER_TEMP/lumo-release-keychains.json" <<'PY'
import json, pathlib, shlex, subprocess, sys
original=shlex.split(subprocess.check_output(['security','list-keychains','-d','user'], text=True))
pathlib.Path(sys.argv[2]).write_text(json.dumps(original))
subprocess.run(['security','list-keychains','-d','user','-s',sys.argv[1],*original],check=True)
PY
    security find-identity -v -p codesigning "$KEYCHAIN" | grep -F -- "$APPLE_SIGNING_IDENTITY" >/dev/null
    rm -f "$CERTIFICATE"
    ;;
  verify)
    : "${APPLE_TEAM_ID:?missing APPLE_TEAM_ID}"
    python3 - "${2:?bundle directory is required}" <<'PY'
import os, pathlib, subprocess, sys, tempfile
root=pathlib.Path(sys.argv[1]); apps=list(root.glob('macos/*.app')); dmgs=list(root.glob('dmg/*.dmg'))
if len(apps)!=1 or not dmgs:
    raise SystemExit('expected exactly one .app and at least one .dmg')
def verify_app(path):
    app=str(path)
    subprocess.run(['codesign','--verify','--deep','--strict','--verbose=2',app],check=True)
    identity=subprocess.run(['codesign','-dv','--verbose=4',app],text=True,capture_output=True,check=True).stderr
    if 'TeamIdentifier='+os.environ['APPLE_TEAM_ID'] not in identity.splitlines():
        raise SystemExit('built app signing team does not match the configured release team')
    subprocess.run(['xcrun','stapler','validate',app],check=True)
    subprocess.run(['spctl','--assess','--type','execute','--verbose=2',app],check=True)
verify_app(apps[0])
# Also verify the app users actually receive inside each read-only mounted DMG.
for dmg in dmgs:
    subprocess.run(['hdiutil','verify',str(dmg)],check=True)
    with tempfile.TemporaryDirectory(prefix='lumo-dmg-verify-') as mount:
        subprocess.run(['hdiutil','attach','-readonly','-nobrowse','-mountpoint',mount,str(dmg)],check=True)
        try:
            shipped=list(pathlib.Path(mount).glob('*.app'))
            if len(shipped)!=1:
                raise SystemExit('DMG must contain exactly one application')
            verify_app(shipped[0])
        finally:
            subprocess.run(['hdiutil','detach',mount],check=True)
print('PASS: app signature, signing team, notarization ticket, Gatekeeper and DMG integrity')
PY
    ;;
  cleanup)
    rm -f "$CERTIFICATE"
    python3 - "$RUNNER_TEMP/lumo-release-keychains.json" <<'PY'
import json, pathlib, subprocess, sys
state=pathlib.Path(sys.argv[1])
if state.exists():
    subprocess.run(['security','list-keychains','-d','user','-s',*json.loads(state.read_text())],check=True)
    state.unlink()
PY
    if [[ -f "$KEYCHAIN" ]]; then security delete-keychain "$KEYCHAIN"; fi
    ;;
  *) echo 'usage: macos-release-signing.sh import|verify BUNDLE_DIR|cleanup' >&2; exit 2 ;;
esac
