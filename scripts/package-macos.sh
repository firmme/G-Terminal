#!/usr/bin/env bash
# Packages the macOS build the way scripts/package.ps1 packages Windows: the
# binary, README, LICENSE, and a third-party notice that carries the license
# files shipping next to each crate.
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"

if [[ "${1:-}" != "--skip-build" ]]; then
  cargo build --release --locked
fi

python3 - "$(uname -m)" <<'PY'
import hashlib, json, os, re, shutil, subprocess, sys, tarfile

arch = sys.argv[1]
meta = json.loads(
    subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1"])
)
version = next(p["version"] for p in meta["packages"] if p["name"] == "g-terminal")
name = f"G-Terminal-{version}-macos-{arch}"
root = os.path.join("dist", name)
shutil.rmtree(root, ignore_errors=True)
os.makedirs(root, exist_ok=True)

shutil.copy2("target/release/g-terminal", root)
for extra in ("README.md", "LICENSE"):
    if os.path.exists(extra):
        shutil.copy2(extra, root)

notices = [
    "G-Terminal third-party dependencies (Cargo.lock, including platform-specific build dependencies).",
    "License text files are included in licenses/ when provided at the crate root.",
]
for package in sorted(meta["packages"], key=lambda p: (p["name"], p["version"])):
    if package["name"] == "g-terminal":
        continue
    notices.append(
        f'\n{package["name"]} {package["version"]}\n'
        f'License: {package.get("license")}\n'
        f'Source: {package.get("repository")}'
    )
    crate_root = os.path.dirname(package["manifest_path"])
    if not os.path.isdir(crate_root):
        continue
    license_files = [
        entry
        for entry in os.listdir(crate_root)
        if re.match(r"^(LICENSE|LICENCE|COPYING|NOTICE)", entry)
        and os.path.isfile(os.path.join(crate_root, entry))
    ]
    if license_files:
        target = os.path.join(root, "licenses", f'{package["name"]}-{package["version"]}')
        os.makedirs(target, exist_ok=True)
        for entry in license_files:
            shutil.copy2(os.path.join(crate_root, entry), target)

with open(os.path.join(root, "THIRD-PARTY-NOTICES.txt"), "w") as handle:
    handle.write("\n".join(notices) + "\n")

archive = os.path.join("dist", name + ".tar.gz")
with tarfile.open(archive, "w:gz") as tar:
    tar.add(root, arcname=name)
digest = hashlib.sha256(open(archive, "rb").read()).hexdigest()
with open(archive + ".sha256", "w") as handle:
    handle.write(f"{digest}  {name}.tar.gz\n")
print(archive)
print(digest)
PY
