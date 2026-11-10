#!/usr/bin/env python3
"""Run real local SFTP/explicit-FTPS acceptance without touching SSH config.

Requires system sshd/ssh-keygen, OpenSSH sftp, openssl and Cargo. Uses temporary
keys/config/data and loopback high ports only. No root or persistent daemon.
"""
import argparse
import getpass
import os
from pathlib import Path
import shutil
import socket
import socketserver
import ssl
import subprocess
import tempfile
import threading
import time


def executable(name, fallback=None):
    value = shutil.which(name) or (fallback if fallback and Path(fallback).is_file() else None)
    if not value:
        raise RuntimeError("Required system executable is missing: " + name)
    return value


def free_port():
    with socket.socket() as connection:
        connection.bind(("127.0.0.1", 0))
        return connection.getsockname()[1]


def run(*args):
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)


class FtpsHandler(socketserver.StreamRequestHandler):
    def handle(self):
        encrypted = False
        private_data = False
        passive = None

        def reply(message):
            self.wfile.write((message + "\r\n").encode())
            self.wfile.flush()

        try:
            reply("220 isolated FTPS fixture")
            while True:
                line = self.rfile.readline(8192)
                if not line:
                    return
                verb, _, argument = line.decode().rstrip("\r\n").partition(" ")
                if verb == "AUTH" and argument == "TLS":
                    reply("234 begin TLS")
                    self.connection = self.server.tls.wrap_socket(self.connection, server_side=True)
                    self.rfile = self.connection.makefile("rb")
                    self.wfile = self.connection.makefile("wb")
                    encrypted = True
                elif not encrypted:
                    reply("530 TLS required")
                elif verb == "PBSZ":
                    reply("200 PBSZ ok")
                elif verb == "PROT" and argument == "P":
                    private_data = True
                    reply("200 private data required")
                elif verb == "USER":
                    reply("331 password required")
                elif verb == "PASS":
                    reply("230 login ok")
                elif verb == "TYPE":
                    reply("200 binary ok")
                elif verb in ("PASV", "EPSV"):
                    if passive:
                        passive.close()
                    passive = socket.socket()
                    passive.bind(("127.0.0.1", 0))
                    passive.listen(1)
                    passive.settimeout(10)
                    port = passive.getsockname()[1]
                    reply(f"229 Entering Extended Passive Mode (|||{port}|)" if verb == "EPSV"
                          else f"227 Entering Passive Mode (127,0,0,1,{port // 256},{port % 256})")
                elif verb in ("STOR", "RETR"):
                    if not private_data or passive is None:
                        reply("534 encrypted data channel required")
                        continue
                    if argument not in ("roundtrip.txt", "large.txt"):
                        reply("550 fixture path rejected")
                        continue
                    reply("150 opening encrypted data")
                    connection, _ = passive.accept()
                    passive.close()
                    passive = None
                    with self.server.tls.wrap_socket(connection, server_side=True) as data:
                        if verb == "STOR":
                            chunks = []
                            while True:
                                chunk = data.recv(32768)
                                if not chunk:
                                    break
                                chunks.append(chunk)
                            self.server.files[argument] = b"".join(chunks)
                        else:
                            data.sendall(self.server.files[argument])
                        # rustls correctly rejects EOF without a TLS shutdown.
                        # Send close_notify before closing the data transport.
                        data.settimeout(3)
                        try:
                            data.unwrap().close()
                        except (OSError, ssl.SSLError):
                            pass
                    reply("226 encrypted transfer complete")
                elif verb == "QUIT":
                    reply("221 goodbye")
                    return
                else:
                    reply("502 command unsupported by fixture")
        except (OSError, ssl.SSLError, KeyError):
            # Wrong certificate/hostname and oversize rejection intentionally
            # abort their sockets. Rust assertions establish the expected error.
            return
        finally:
            if passive:
                passive.close()


class FtpsServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", help="Run an already built secure_transfer test binary")
    args = parser.parse_args()
    sshd = executable("sshd", "/usr/sbin/sshd")
    ssh_keygen = executable("ssh-keygen")
    executable("sftp")
    openssl = executable("openssl")
    repository = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="lumo secure transfer ") as directory:
        root = Path(directory).resolve()
        for name in ("host", "wrong_host", "client"):
            run(ssh_keygen, "-q", "-t", "ed25519", "-N", "", "-f", str(root / name))
        port = free_port()
        host = (root / "host.pub").read_text().split()
        wrong = (root / "wrong_host.pub").read_text().split()
        (root / "known_hosts").write_text(f"[127.0.0.1]:{port} {host[0]} {host[1]}\n")
        (root / "wrong_known_hosts").write_text(f"[127.0.0.1]:{port} {wrong[0]} {wrong[1]}\n")
        user = getpass.getuser()
        (root / "sshd_config").write_text(
            f'Port {port}\nListenAddress 127.0.0.1\nHostKey "{root / "host"}"\n'
            f'PidFile "{root / "sshd.pid"}"\nAuthorizedKeysFile "{root / "client.pub"}"\n'
            f"AllowUsers {user}\nStrictModes no\nUsePAM no\nPasswordAuthentication no\n"
            "KbdInteractiveAuthentication no\nPubkeyAuthentication yes\nPermitRootLogin prohibit-password\n"
            "Subsystem sftp internal-sftp\nLogLevel ERROR\n")
        run(sshd, "-t", "-f", str(root / "sshd_config"))
        log = (root / "sshd.log").open("wb")
        process = subprocess.Popen([sshd, "-D", "-e", "-f", str(root / "sshd_config")],
                                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=log)
        server = None
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise RuntimeError("Temporary sshd failed: " + (root / "sshd.log").read_text())
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=.1):
                        break
                except OSError:
                    time.sleep(.05)
            else:
                raise RuntimeError("Temporary sshd did not start")
            # OpenSSL on macOS may be LibreSSL: use a config file instead of
            # -addext so hostname SAN validation is exercised everywhere.
            (root / "ca.cnf").write_text("[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=LumoFixtureRoot\n[ext]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n")
            run(openssl, "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                "-keyout", str(root / "ca.key"), "-out", str(root / "ca.crt"), "-config", str(root / "ca.cnf"))
            (root / "openssl.cnf").write_text("[req]\ndistinguished_name=dn\nprompt=no\n[dn]\nCN=localhost\n[ext]\nsubjectAltName=DNS:localhost\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n")
            run(openssl, "req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", str(root / "ftps.key"),
                "-out", str(root / "ftps.csr"), "-config", str(root / "openssl.cnf"))
            run(openssl, "x509", "-req", "-in", str(root / "ftps.csr"), "-CA", str(root / "ca.crt"), "-CAkey", str(root / "ca.key"),
                "-set_serial", "1", "-days", "1", "-out", str(root / "ftps.crt"), "-extfile", str(root / "openssl.cnf"), "-extensions", "ext")
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.minimum_version = ssl.TLSVersion.TLSv1_2
            context.load_cert_chain(root / "ftps.crt", root / "ftps.key")
            server = FtpsServer(("127.0.0.1", 0), FtpsHandler)
            server.tls = context
            server.files = {"large.txt": b"large" * 10000}
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            environment = dict(os.environ, LUMO_SFTP_PORT=str(port), LUMO_SFTP_TEST_ROOT=str(root),
                               LUMO_SFTP_USER=user, LUMO_FTPS_PORT=str(server.server_address[1]),
                               CARGO_INCREMENTAL="0")
            command = ([args.test_binary] if args.test_binary else
                       ["cargo", "test", "-p", "lumo-actions", "--test", "secure_transfer", "--"])
            subprocess.run(command + ["--ignored", "--nocapture", "live_"], cwd=repository, env=environment, check=True)
            print("Real SFTP and explicit FTPS acceptance passed")
        finally:
            if server:
                server.shutdown()
                server.server_close()
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            log.close()


if __name__ == "__main__":
    main()
