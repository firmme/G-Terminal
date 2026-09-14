param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
Set-Location $projectRoot
if (-not $SkipBuild) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$PSScriptRoot\build.ps1" -Release
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
}
$metadataJson = & cargo metadata --locked --format-version 1
if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo dependency metadata.' }
$metadata = $metadataJson | ConvertFrom-Json
$version = ($metadata.packages | Where-Object { $_.name -eq 'g-terminal' }).version
$packageName = "G-Terminal-$version-windows-x64"
$packageDir = Join-Path $projectRoot "dist\$packageName"
New-Item -ItemType Directory -Path $packageDir -Force | Out-Null
Copy-Item -LiteralPath "$projectRoot\target\release\g-terminal.exe" -Destination $packageDir -Force
Copy-Item -LiteralPath "$projectRoot\README.md", "$projectRoot\LICENSE" -Destination $packageDir -Force
$notices = [System.Collections.Generic.List[string]]::new()
$notices.Add('G-Terminal third-party dependencies (Cargo.lock, including platform-specific build dependencies).')
$notices.Add('License text files are included in licenses/ when provided at the crate root.')
foreach ($package in $metadata.packages | Sort-Object name,version) {
    if ($package.name -eq 'g-terminal') { continue }
    $notices.Add("`n$($package.name) $($package.version)`nLicense: $($package.license)`nSource: $($package.repository)")
    $crateRoot = Split-Path $package.manifest_path -Parent
    $licenseFiles = Get-ChildItem -LiteralPath $crateRoot -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)' }
    if ($licenseFiles) {
        $licenseDir = Join-Path $packageDir "licenses\$($package.name)-$($package.version)"
        New-Item -ItemType Directory -Path $licenseDir -Force | Out-Null
        foreach ($licenseFile in $licenseFiles) { Copy-Item -LiteralPath $licenseFile.FullName -Destination $licenseDir -Force }
    }
}
[System.IO.File]::WriteAllLines((Join-Path $packageDir 'THIRD-PARTY-NOTICES.txt'), $notices)
$archive = Join-Path $projectRoot "dist\$packageName.zip"
Compress-Archive -LiteralPath $packageDir -DestinationPath $archive -Force
$hash = Get-FileHash -LiteralPath $archive -Algorithm SHA256
[System.IO.File]::WriteAllText("$archive.sha256", "$($hash.Hash.ToLowerInvariant())  $packageName.zip`n")
Get-Item -LiteralPath $archive | Select-Object FullName,Length

