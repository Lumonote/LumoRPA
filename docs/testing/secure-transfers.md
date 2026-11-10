# Secure remote file transfers

`sftp.upload` and `sftp.download` require the system OpenSSH `sftp` executable.
They use public-key authentication in BatchMode. Supply `identity_file` and an
independently verified `known_hosts` file explicitly; unknown/changed host keys
fail. Password and interactive passphrase authentication are intentionally not
accepted. User SSH configuration, global known_hosts, SSH agent, proxy commands,
forwards, password prompts and automatic key updates are disabled.

```yaml
- id: download
  action: sftp.download
  with:
    host: sftp.example.com
    port: 22
    username: service
    identity_file: /authorized/keys/id_ed25519
    known_hosts: /authorized/keys/known_hosts
    remote_path: /incoming/report.csv
    local_path: /authorized/data/report.csv
    max_bytes: 104857600
    timeout_ms: 60000
```

Grant `network` for the server, `fs.read` for the key/known_hosts (and upload
source), and `fs.write` for the download directory. Destination directories must
already exist. Downloads use a private staging file in the destination directory
and publish only after a successful, bounded transfer. A failed host-key check,
oversize response, cancellation or timeout preserves the previous destination.
Stop/cancel kills the OpenSSH process tree and reaps the child. Uploads first use
a unique remote temporary filename then rename to the requested path. If the
server rejects rename or the connection dies, the server may retain that remote
temporary file; the result is a failure rather than an asserted successful upload.

Existing `ftp.upload`/`ftp.download` support `tls: explicit`. AUTH TLS and
certificate/hostname verification must succeed before login. Both control and
data connections use TLS (`PROT P`); there is no downgrade to plaintext. System
CA roots are used, and `ca_cert` may add an explicitly authorized PEM CA bundle.
It does not disable hostname verification. `timeout_ms` defaults to 60000 for
explicit TLS. TLS steps use a fresh secure connection rather than reusing a
plaintext `ftp` resource session. Omit `tls` to keep legacy plaintext FTP behavior.

```yaml
- id: ftps_download
  action: ftp.download
  with:
    host: files.example.com
    port: 21
    username: service
    password: ${{ vault.ftp.password }}
    remote_path: report.csv
    local_path: /authorized/data/report.csv
    tls: explicit
    ca_cert: /authorized/keys/private-ca.pem
    timeout_ms: 60000
```

## Acceptance

`cargo test -p lumo-actions --test secure_transfer` checks missing authorization,
path/argument injection, explicit host-key requirements and TLS downgrade
rejection without contacting external services.

`python3 scripts/secure-transfer-e2e.py` starts an isolated OpenSSH daemon and a
TLS FTP fixture on loopback high ports with temporary keys, certificates and
configuration. It runs real production actions against both servers: Unicode
roundtrips with paths containing spaces, encrypted FTPS data channels,
wrong-host-key/untrusted-certificate/hostname rejection and oversize preservation.
It never modifies the user's SSH configuration or starts a system daemon.
Dependencies: OpenSSH server/client, OpenSSL, Python 3 and Cargo. Linux may need
the distribution's `/run/sshd` privilege-separation directory created by its
package setup. The `secure transfer acceptance` CI workflow runs these checks on
Ubuntu. A missing executable or blocked loopback port fails the runner.
