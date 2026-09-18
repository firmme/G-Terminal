#!/usr/bin/env bash
# Packages the macOS build the way scripts/package.ps1 packages Windows: the
# app, README, LICENSE, and a third-party notice that carries the license files
# shipping next to each crate.
#
# The executable is wrapped in a .app bundle. A bare binary double-clicked in
# Finder is handed to Terminal, which then runs the program in a console
# window; a bundle is what LaunchServices recognises as a windowed application,
# so it opens without that Terminal window.
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

# The bundle: Contents/MacOS holds the executable, Contents/Resources the
# documentation and third-party notices.
bundle = os.path.join(root, "G-Terminal.app")
macos_dir = os.path.join(bundle, "Contents", "MacOS")
resources = os.path.join(bundle, "Contents", "Resources")
os.makedirs(macos_dir, exist_ok=True)
os.makedirs(resources, exist_ok=True)

binary = os.path.join(macos_dir, "g-terminal")
shutil.copy2("target/release/g-terminal", binary)
os.chmod(binary, 0o755)

# The Finder / Dock icon. The mark is code-drawn, so the app itself exports the
# PNG set and `iconutil` packs it into the bundle's `.icns`.
iconset = os.path.join("dist", name + ".iconset")
shutil.rmtree(iconset, ignore_errors=True)
subprocess.check_call([binary, "--export-iconset", iconset])
subprocess.check_call(
    ["iconutil", "-c", "icns", iconset, "-o", os.path.join(resources, "G-Terminal.icns")]
)
shutil.rmtree(iconset, ignore_errors=True)

with open(os.path.join(bundle, "Contents", "Info.plist"), "w") as handle:
    handle.write(f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>G-Terminal</string>
    <key>CFBundleDisplayName</key><string>G-Terminal</string>
    <key>CFBundleIdentifier</key><string>dev.gterminal.G-Terminal</string>
    <key>CFBundleExecutable</key><string>g-terminal</string>
    <key>CFBundleIconFile</key><string>G-Terminal</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>{version}</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
""")

# A copy of the docs sits beside the bundle too, for anyone who unpacks it just
# to read them.
for extra in ("README.md", "LICENSE"):
    if os.path.exists(extra):
        shutil.copy2(extra, root)
        shutil.copy2(extra, resources)

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
        target = os.path.join(resources, "licenses", f'{package["name"]}-{package["version"]}')
        os.makedirs(target, exist_ok=True)
        for entry in license_files:
            shutil.copy2(os.path.join(crate_root, entry), target)

with open(os.path.join(resources, "THIRD-PARTY-NOTICES.txt"), "w") as handle:
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
