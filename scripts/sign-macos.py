#!/usr/bin/env python3
"""Create/reuse a private local signing identity and sign the finished bundle.

No Apple account or access to login credentials required. Initialization adds
code-signing trust for this certificate to the current user's trust settings.
Keep the signing directory backed up privately: replacing its certificate changes
the application's identity. Never include it in a release or source repository.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import secrets
import shlex
import shutil
import subprocess
import ssl
import sys
import tempfile

APP_ID = "dev.gterminal.G-Terminal"
SIGNING_DIR = Path(os.environ.get(
    "GTERMINAL_SIGNING_DIR",
    str(Path.home() / "Library/Application Support/G-Terminal/signing"),
)).expanduser().resolve()


def run(args):
    # Avoid including command arguments in errors: security receives a password.
    result = subprocess.run([str(a) for a in args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"{Path(args[0]).name} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def init():
    if SIGNING_DIR.exists():
        identity = load_identity()
        ensure_trust(identity)
        print(f"Reusing signing identity in {SIGNING_DIR}")
        return
    openssl = shutil.which("openssl")
    if not openssl:
        raise RuntimeError("OpenSSL is required to create the signing identity")
    SIGNING_DIR.mkdir(parents=True, mode=0o700)
    os.chmod(SIGNING_DIR, 0o700)
    password = secrets.token_hex(32)
    password_file = SIGNING_DIR / "keychain-password"
    password_file.write_text(password)
    os.chmod(password_file, 0o600)
    keychain = SIGNING_DIR / "release.keychain-db"
    certificate = SIGNING_DIR / "certificate.pem"
    # A partial setup deliberately stays on disk: never silently replace a key.
    with tempfile.TemporaryDirectory(prefix="gterminal-signing-") as temp:
        temp = Path(temp)
        key = temp / "key.pem"
        archive = temp / "identity.p12"
        config = temp / "certificate.cnf"
        # Config syntax works with both macOS LibreSSL and Homebrew OpenSSL.
        config.write_text("[req]\ndistinguished_name=dn\n[dn]\n[codesign]\n"
                          "basicConstraints=critical,CA:FALSE\n"
                          "keyUsage=critical,digitalSignature\n"
                          "extendedKeyUsage=critical,codeSigning\n")
        run([openssl, "req", "-x509", "-newkey", "rsa:3072", "-nodes",
             "-sha256", "-days", "7300", "-subj",
             "/CN=G-Terminal Local Release Signing/",
             "-config", config, "-extensions", "codesign",
             "-keyout", key, "-out", certificate])
        os.chmod(key, 0o600)
        # macOS Security supports this PKCS12 format, including older macOS.
        run([openssl, "pkcs12", "-export", "-inkey", key, "-in", certificate,
             "-out", archive, "-passout", f"file:{password_file}",
             "-keypbe", "PBE-SHA1-3DES", "-certpbe", "PBE-SHA1-3DES",
             "-macalg", "sha1"])
        run(["/usr/bin/security", "create-keychain", "-p", password, keychain])
        run(["/usr/bin/security", "unlock-keychain", "-p", password, keychain])
        run(["/usr/bin/security", "import", archive, "-k", keychain,
             "-P", password, "-T", "/usr/bin/codesign"])
    fingerprint = run([openssl, "x509", "-in", certificate, "-noout",
                       "-fingerprint", "-sha1"]).split("=", 1)[1].replace(":", "")
    (SIGNING_DIR / "identity.json").write_text(json.dumps({
        "certificate_sha1": fingerprint,
        "bundle_identifier": APP_ID,
    }, indent=2) + "\n")
    os.chmod(keychain, 0o600)
    ensure_trust(load_identity())
    print(f"Created local signing identity {fingerprint} in {SIGNING_DIR}")


def ensure_trust(identity):
    keychain = SIGNING_DIR / "release.keychain-db"
    # codesign also needs the private key's keychain in the user's search list.
    # Preserve existing entries and the user's default keychain.
    search = shlex.split(run(["/usr/bin/security", "list-keychains", "-d", "user"]))
    if str(keychain) not in search:
        run(["/usr/bin/security", "list-keychains", "-d", "user", "-s", *search, keychain])
    valid = run(["/usr/bin/security", "find-identity", "-v", "-p", "codesigning", keychain])
    if identity["certificate_sha1"] not in valid:
        # User scope only; code-signing policy only. No SSL/root trust is added.
        run(["/usr/bin/security", "add-trusted-cert", "-r", "trustRoot",
             "-p", "codeSign", SIGNING_DIR / "certificate.pem"])


def load_identity():
    metadata = SIGNING_DIR / "identity.json"
    if not metadata.is_file():
        raise RuntimeError("Signing identity missing/incomplete. Run scripts/sign-macos.py "
                           "--init once; never regenerate a previously used identity.")
    identity = json.loads(metadata.read_text())
    if identity["bundle_identifier"] != APP_ID:
        raise RuntimeError("Unexpected signing bundle identifier")
    fingerprint = identity["certificate_sha1"]
    if not isinstance(fingerprint, str) or not re.fullmatch(r"[0-9A-F]{40}", fingerprint):
        raise RuntimeError("Invalid signing certificate fingerprint")
    certificate = ssl.PEM_cert_to_DER_cert((SIGNING_DIR / "certificate.pem").read_text())
    if hashlib.sha1(certificate).hexdigest().upper() != fingerprint:
        raise RuntimeError("Signing certificate differs from the pinned identity")
    for private in (SIGNING_DIR, SIGNING_DIR / "keychain-password",
                    SIGNING_DIR / "release.keychain-db"):
        if private.stat().st_mode & 0o077:
            raise RuntimeError(f"Signing material must be accessible only to its owner: {private}")
    return identity


def sign(bundle):
    identity = load_identity()
    with (bundle / "Contents/Info.plist").open("rb") as handle:
        if plistlib.load(handle).get("CFBundleIdentifier") != APP_ID:
            raise RuntimeError("Bundle identifier does not match the signing identity")
    password = (SIGNING_DIR / "keychain-password").read_text()
    keychain = SIGNING_DIR / "release.keychain-db"
    run(["/usr/bin/security", "unlock-keychain", "-p", password, keychain])
    fingerprint = identity["certificate_sha1"]
    # Pin the certificate rather than the executable hash or a certificate name.
    # The requirement stays identical when code/resources change across releases.
    requirement = f'identifier "{APP_ID}" and anchor = H"{fingerprint}"'
    run(["/usr/bin/codesign", "--force", "--sign", fingerprint,
         "--keychain", keychain, "--identifier", APP_ID, "--timestamp=none",
         "--requirements", "=designated => " + requirement, bundle])
    run(["/usr/bin/codesign", "--verify", "--strict", "--verbose=2", bundle])
    run(["/usr/bin/codesign", "--verify", "--strict", "-R", "=" + requirement, bundle])
    print(f"Signed and verified {bundle} with {fingerprint}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--init", action="store_true", help="create once, reuse thereafter")
    parser.add_argument("--check", action="store_true", help="validate existing signing material")
    parser.add_argument("bundle", nargs="?", type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        if sys.platform != "darwin":
            raise RuntimeError("macOS signing requires macOS")
        if args.init:
            init()
        if args.check:
            load_identity()
            print("Signing identity and private file permissions verified")
        if args.bundle:
            sign(args.bundle.resolve())
        if not args.init and not args.check and not args.bundle:
            parser.error("provide --init, --check or a finished .app bundle")
    except (RuntimeError, OSError, ValueError, KeyError) as error:
        sys.exit(str(error))
