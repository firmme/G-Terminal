param(
    [ValidateSet('check', 'test', 'build', 'run', 'clippy')][string]$Action = 'build',
    [switch]$Release,
    # Optional rustup toolchain name, e.g. 'stable-x86_64-pc-windows-gnu'. When omitted, the
    # active default toolchain decides which environment (MSVC or GNU) gets prepared.
    [string]$Toolchain = ''
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

function Get-RustHostTriple([string]$Toolchain) {
    $rustcArgs = @()
    if ($Toolchain) { $rustcArgs += "+$Toolchain" }
    $rustcArgs += '-vV'
    $hostLine = & rustc @rustcArgs 2>$null | Where-Object { $_ -like 'host:*' } | Select-Object -First 1
    if (-not $hostLine) {
        if ($Toolchain) {
            throw "Toolchain '$Toolchain' is not usable. Check 'rustup toolchain list', or install it with 'rustup toolchain install $Toolchain'."
        }
        throw 'rustc not found on PATH. Install Rust from https://rustup.rs and reopen the shell.'
    }
    return ($hostLine -replace '^host:\s*', '').Trim()
}

function Get-MsvcArch([string]$Triple) {
    # MSVC spells its toolchain directories x64/x86/arm64; Rust spells them x86_64/i686/aarch64.
    # Both Host<arch> and <arch> use the same spelling, so one name covers both path segments.
    switch -Regex ($Triple) {
        '^x86_64-' { return 'x64' }
        '^i[3-6]86-' { return 'x86' }
        '^aarch64-' { return 'arm64' }
        default { throw "Unsupported MSVC host architecture in '$Triple'." }
    }
}

function Test-MsvcEnvironment {
    # MSVC is already usable when this shell came from a Developer Command Prompt, or when
    # link.exe on PATH is a real MSVC linker. Note that Git for Windows ships a coreutils
    # `link.exe` (the file-linking utility) which must not be mistaken for the linker.
    if ($env:VSCMD_VER) { return $true }
    $link = Get-Command link.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $link) { return $false }
    return $link.Source -match '\\VC\\Tools\\MSVC\\'
}

function Find-MsvcLinker([string]$Arch, [string]$HostArch) {
    $relative = "VC\Tools\MSVC\*\bin\Host$HostArch\$Arch\link.exe"

    # vswhere is the supported, fast way to locate Visual Studio and Build Tools installs.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path -LiteralPath $vswhere) {
        $found = & $vswhere -latest -prerelease -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -find $relative 2>$null
        $linker = $found | Where-Object { $_ -and (Test-Path -LiteralPath $_) } |
            Sort-Object -Descending | Select-Object -First 1
        if ($linker) { return Get-Item -LiteralPath $linker }
    }

    # Fallback for Build Tools installations that are missing vswhere registration.
    $vsRoots = @("${env:ProgramFiles}\Microsoft Visual Studio", "${env:ProgramFiles(x86)}\Microsoft Visual Studio")
    $matches = $vsRoots | Where-Object { Test-Path -LiteralPath $_ } |
        ForEach-Object { Get-ChildItem -Path (Join-Path $_ "*\*\VC\Tools\MSVC\*\bin\Host$HostArch\$Arch\link.exe") -ErrorAction SilentlyContinue }
    return $matches | Sort-Object FullName -Descending | Select-Object -First 1
}

function Find-WindowsSdk([string]$Arch) {
    # Pick the newest SDK that actually ships libraries for this architecture.
    $sdkRoot = "${env:ProgramFiles(x86)}\Windows Kits\10"
    $libRoot = Join-Path $sdkRoot 'Lib'
    if (-not (Test-Path -LiteralPath $libRoot)) { return $null }
    $version = Get-ChildItem -LiteralPath $libRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object {
            (Test-Path -LiteralPath (Join-Path $_.FullName "um\$Arch")) -and
            (Test-Path -LiteralPath (Join-Path $_.FullName "ucrt\$Arch"))
        } |
        ForEach-Object { [pscustomobject]@{ Name = $_.Name; Parsed = $_.Name -as [version] } } |
        Sort-Object Parsed -Descending | Select-Object -First 1
    if (-not $version) { return $null }
    return [pscustomobject]@{ Root = $sdkRoot; Version = $version.Name }
}

function Initialize-MsvcEnvironment([string]$Triple) {
    if (Test-MsvcEnvironment) { return }

    $arch = Get-MsvcArch $Triple
    $linker = Find-MsvcLinker -Arch $arch -HostArch $arch
    if (-not $linker) {
        throw @"
No Visual Studio C++ toolchain found, but the active Rust toolchain targets MSVC ($Triple).

Install either one, then reopen the shell:

  # Option A - MSVC build tools (matches the released Windows x64 package):
  winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"

  # Option B - GNU toolchain, if MSVC is unavailable on this machine:
  rustup toolchain install stable-$(($Triple -replace '-msvc$', '-gnu'))
  # then build with:  ./scripts/build.ps1 -Action build -Release -Toolchain stable-$(($Triple -replace '-msvc$', '-gnu'))
"@
    }

    $sdk = Find-WindowsSdk -Arch $arch
    if (-not $sdk) {
        throw "Found an MSVC C++ compiler at '$($linker.FullName)' but no Windows 10/11 SDK with $arch libraries. Install the 'Windows SDK' component in the Visual Studio Installer."
    }

    # ...\VC\Tools\MSVC\<version>\bin\Host<arch>\<arch>\link.exe -> ...\VC\Tools\MSVC\<version>
    $msvcRoot = Split-Path (Split-Path (Split-Path $linker.DirectoryName -Parent) -Parent) -Parent
    $env:PATH = "$($linker.DirectoryName);$(Join-Path $sdk.Root "bin\$($sdk.Version)\$arch");$env:PATH"
    $env:LIB = "$(Join-Path $msvcRoot "lib\$arch");$(Join-Path $sdk.Root "Lib\$($sdk.Version)\um\$arch");$(Join-Path $sdk.Root "Lib\$($sdk.Version)\ucrt\$arch");$env:LIB"
    $env:INCLUDE = "$(Join-Path $msvcRoot 'include');$(Join-Path $sdk.Root "Include\$($sdk.Version)\ucrt");$(Join-Path $sdk.Root "Include\$($sdk.Version)\shared");$(Join-Path $sdk.Root "Include\$($sdk.Version)\um");$env:INCLUDE"
}

function Initialize-GnuEnvironment([string]$Triple) {
    # The GNU toolchain links itself; MSVC is irrelevant here. Only flag a missing linker early,
    # so the failure names the missing prerequisite instead of surfacing deep inside a build script.
    if (Get-Command gcc -ErrorAction SilentlyContinue) { return }
    Write-Warning "Active Rust toolchain targets GNU ($Triple) but gcc is not on PATH. Install mingw-w64 (e.g. 'winget install --id MSYS2.MSYS2', then 'pacman -S mingw-w64-x86_64-toolchain')."
}

$hostTriple = Get-RustHostTriple $Toolchain
switch -Regex ($hostTriple) {
    '-windows-msvc$' { Initialize-MsvcEnvironment $hostTriple }
    '-windows-gnu$' { Initialize-GnuEnvironment $hostTriple }
}

$cargoArgs = @()
if ($Toolchain) { $cargoArgs += "+$Toolchain" }
$cargoArgs += $Action, '--locked'
if ($Release) { $cargoArgs += '--release' }
if ($Action -eq 'clippy') { $cargoArgs += @('--all-targets', '--', '-D', 'warnings') }
& cargo @cargoArgs
exit $LASTEXITCODE
