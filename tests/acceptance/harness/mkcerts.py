#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Make a throwaway CA and a server certificate for the PostgreSQL container of the acceptance tests.

The harness runs this script inside the runner image, where the package `cryptography` is installed.
Use: python3 mkcerts.py OUT_DIR SERVER_NAME

It writes OUT_DIR/ca.crt, OUT_DIR/server.crt and OUT_DIR/server.key (ECDSA P-256, valid for one day).
The CA file has one PEM block `CERTIFICATE` and no other text (6.13).
"""
import datetime
import os
import sys

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID


def name(common_name: str) -> x509.Name:
    return x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, common_name)])


def main(out: str, server_name: str) -> None:
    now = datetime.datetime.now(datetime.timezone.utc)
    start = now - datetime.timedelta(minutes=5)
    end = now + datetime.timedelta(days=1)

    ca_key = ec.generate_private_key(ec.SECP256R1())
    ca_cert = (
        x509.CertificateBuilder()
        .subject_name(name("pupitre acceptance CA"))
        .issuer_name(name("pupitre acceptance CA"))
        .public_key(ca_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(start)
        .not_valid_after(end)
        .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
        .add_extension(x509.KeyUsage(True, False, False, False, False, True, True, False, False), critical=True)
        .sign(ca_key, hashes.SHA256())
    )

    server_key = ec.generate_private_key(ec.SECP256R1())
    server_cert = (
        x509.CertificateBuilder()
        .subject_name(name(server_name))
        .issuer_name(ca_cert.subject)
        .public_key(server_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(start)
        .not_valid_after(end)
        .add_extension(x509.SubjectAlternativeName([x509.DNSName(server_name)]), critical=False)
        .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
        .add_extension(x509.ExtendedKeyUsage([x509.oid.ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
        .sign(ca_key, hashes.SHA256())
    )

    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "ca.crt"), "wb") as handle:
        handle.write(ca_cert.public_bytes(serialization.Encoding.PEM))
    with open(os.path.join(out, "server.crt"), "wb") as handle:
        handle.write(server_cert.public_bytes(serialization.Encoding.PEM))
    key_path = os.path.join(out, "server.key")
    with open(os.open(key_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "wb") as handle:
        handle.write(
            server_key.private_bytes(
                serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()
            )
        )


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("use: mkcerts.py OUT_DIR SERVER_NAME", file=sys.stderr)
        sys.exit(2)
    main(sys.argv[1], sys.argv[2])
