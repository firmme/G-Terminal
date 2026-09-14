param(
    [ValidateSet('check', 'test', 'build', 'run', 'clippy')][string]$Action = 'build',
    [switch]$Release
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

# Also support Build Tools installations that are missing vswhere registration.
if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
    $vsRoots = @("${env:ProgramFiles}\Microsoft Visual Studio", "${env:ProgramFiles(x86)}\Microsoft Visual Studio")
    $linker = $vsRoots | Where-Object { Test-Path -LiteralPath $_ } |
        ForEach-Object { Get-ChildItem -LiteralPath $_ -Filter link.exe -Recurse -ErrorAction SilentlyContinue } |
        Where-Object { $_.FullName -match 'Hostx64\\x64\\link.exe$' } |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $linker) { throw 'Install Visual Studio Build Tools with Desktop development with C++ and Windows SDK.' }
    $msvcRoot = Split-Path (Split-Path (Split-Path $linker.DirectoryName -Parent) -Parent) -Parent
    $sdkRoot = "${env:ProgramFiles(x86)}\Windows Kits\10"
    $sdkVersion = Get-ChildItem "$sdkRoot\Lib" -Directory | Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty Name
    $env:PATH = "$($linker.DirectoryName);$sdkRoot\bin\$sdkVersion\x64;$env:PATH"
    $env:LIB = "$msvcRoot\lib\x64;$sdkRoot\Lib\$sdkVersion\um\x64;$sdkRoot\Lib\$sdkVersion\ucrt\x64;$env:LIB"
    $env:INCLUDE = "$msvcRoot\include;$sdkRoot\Include\$sdkVersion\ucrt;$sdkRoot\Include\$sdkVersion\shared;$sdkRoot\Include\$sdkVersion\um;$env:INCLUDE"
}
$cargoArgs = @($Action, '--locked')
if ($Release) { $cargoArgs += '--release' }
if ($Action -eq 'clippy') { $cargoArgs += @('--all-targets', '--', '-D', 'warnings') }
& cargo @cargoArgs
exit $LASTEXITCODE

