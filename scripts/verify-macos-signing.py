#!/usr/bin/env python3
"""Verify that two different signed builds share an identity, and detect tampering.

Uses disposable app bundles; never reads or changes actual OneDrive credentials.
Run after `python3 scripts/sign-macos.py --init`.
"""
import plistlib
from pathlib import Path
import subprocess
import sys
import tempfile

SIGNER = Path(__file__).with_name("sign-macos.py")
APP_ID = "dev.gterminal.G-Terminal"
SOURCE = r'''
#include <Security/Security.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    SecKeychainRef keychain = NULL;
    OSStatus status;
    const char *password = "disposable-test-keychain";
    const char *service = "G-Terminal.SigningTest";
    const char *account = "dummy-account";
    const char *secret = "dummy-secret-not-a-real-credential";
    SecKeychainSetUserInteractionAllowed(false);
    if (!strcmp(argv[2], "create")) {
        status = SecKeychainCreate(argv[1], (UInt32)strlen(password), password,
                                   false, NULL, &keychain);
        if (!status) status = SecKeychainAddGenericPassword(keychain,
            (UInt32)strlen(service), service, (UInt32)strlen(account), account,
            (UInt32)strlen(secret), secret, NULL);
    } else {
        status = SecKeychainOpen(argv[1], &keychain);
        if (!status) status = SecKeychainUnlock(keychain,
            (UInt32)strlen(password), password, true);
        for (int i = 0; !status && i < 3; ++i) {
            UInt32 length = 0;
            void *data = NULL;
            status = SecKeychainFindGenericPassword(keychain,
                (UInt32)strlen(service), service, (UInt32)strlen(account), account,
                &length, &data, NULL);
            if (!status && (length != strlen(secret) || memcmp(data, secret, length)))
                status = errSecDecode;
            if (data) SecKeychainItemFreeContent(NULL, data);
        }
    }
    if (keychain) CFRelease(keychain);
    if (status) fprintf(stderr, "Keychain test failed: %d (build %d)\n", (int)status, VERSION);
    return status ? 1 : 0;
}
'''


def run(args):
    return subprocess.run(args, check=True, capture_output=True, text=True)


with tempfile.TemporaryDirectory(prefix="gterminal-signing-check-") as temp:
    temp = Path(temp)
    requirements = []
    hashes = []
    for version in (1, 2):
        bundle = temp / f"v{version}" / "G-Terminal.app"
        contents = bundle / "Contents"
        executable = contents / "MacOS/g-terminal"
        executable.parent.mkdir(parents=True)
        (contents / "Resources").mkdir()
        (contents / "Resources/version.txt").write_text(str(version))
        (contents / "Info.plist").write_bytes(plistlib.dumps({
            "CFBundleIdentifier": APP_ID,
            "CFBundleExecutable": "g-terminal",
            "CFBundlePackageType": "APPL",
            "CFBundleVersion": str(version),
        }))
        source = temp / f"v{version}.c"
        source.write_text(SOURCE)
        run(["/usr/bin/cc", "-Wno-deprecated-declarations", f"-DVERSION={version}",
             str(source), "-framework", "Security", "-framework", "CoreFoundation",
             "-o", str(executable)])
        run([sys.executable, str(SIGNER), str(bundle)])
        display = run(["/usr/bin/codesign", "-d", "-r-", str(bundle)])
        requirement = display.stdout + display.stderr
        requirement = next(line.removeprefix("designated => ")
                           for line in requirement.splitlines()
                           if line.startswith("designated => "))
        requirements.append(requirement)
        details = run(["/usr/bin/codesign", "-d", "--verbose=4", str(bundle)]).stderr
        hashes.append(next(line for line in details.splitlines() if line.startswith("CDHash=")))
        run(["/usr/bin/codesign", "--verify", "--strict", "-R", "=" + requirements[0], str(bundle)])
        if version == 1:
            run([str(executable), str(temp / "test.keychain"), "create"])
        # UI is disabled: a prompt would fail this test instead of asking the user.
        run([str(executable), str(temp / "test.keychain"), "read"])
    assert requirements[0] == requirements[1], "Signing requirement changed across builds"
    assert hashes[0] != hashes[1], "Test did not change the executable"
    (contents / "Resources/version.txt").write_text("tampered")
    result = subprocess.run(["/usr/bin/codesign", "--verify", "--strict", str(bundle)],
                            capture_output=True)
    assert result.returncode != 0, "Resource tampering was not detected"
print("PASS: repeated credential reads and a changed build need no authorization UI; "
      "certificate-pinned identity is stable and resource tampering is rejected")
