param(
    # Delete for real. Without this the script only reports what it would do.
    [switch]$Apply,
    # Keep the artifacts of other installed Rust toolchains (for example the GNU
    # fallback). They can never be reused by the active toolchain, but dropping
    # them costs a full rebuild if that toolchain is used later.
    [switch]$KeepOtherToolchains,
    # Incremental sessions untouched for this long are from a build
    # configuration the current one no longer uses. Everything the recent
    # builds compiled stays, so the next build keeps its speed.
    [double]$KeepIncrementalDays = 1
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
$target = Join-Path $projectRoot 'target'
if (-not (Test-Path -LiteralPath $target)) {
    Write-Output 'There is no target directory to prune.'
    return
}

function Get-DirectorySize($path) {
    $sum = (Get-ChildItem -LiteralPath $path -Recurse -Force -File -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) { return 0 }
    return $sum
}

$freed = [long]0
$removed = 0
foreach ($profile in Get-ChildItem -LiteralPath $target -Directory) {
    # A profile directory is the one holding a fingerprint database; anything
    # else (`tmp`) is none of this script's business.
    $fingerprints = Join-Path $profile.FullName '.fingerprint'
    if (-not (Test-Path -LiteralPath $fingerprints)) { continue }

    # Each fingerprint directory records the rustc that produced it. A unit
    # built by another rustc can never match again, so those directories — and
    # the artifacts named after them — are dead weight for this toolchain.
    $units = foreach ($dir in Get-ChildItem -LiteralPath $fingerprints -Directory) {
        $rustc = @(Get-ChildItem -LiteralPath $dir.FullName -Filter *.json -ErrorAction SilentlyContinue |
            ForEach-Object { (Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json).rustc } |
            Select-Object -Unique)
        [pscustomobject]@{
            Dir   = $dir
            Hash  = ($dir.Name -split '-')[-1]
            Rustc = $rustc
            When  = $dir.LastWriteTime
        }
    }
    $newest = $units | Sort-Object When -Descending | Select-Object -First 1
    $activeRustc = if ($newest) { ($newest.Rustc -join ',') } else { '' }

    $stale = @()
    if (-not $KeepOtherToolchains -and $activeRustc) {
        # Directories with no fingerprint at all are left alone: they are not
        # provably dead.
        $stale = $units | Where-Object {
            $_.Rustc.Count -gt 0 -and (($_.Rustc -join ',') -ne $activeRustc)
        }
    }
    $live = @{}
    foreach ($unit in $units) {
        if ($stale -notcontains $unit) { $live[$unit.Hash] = $true }
    }

    # An artifact whose hash no longer has a fingerprint will never be reused.
    $deps = Join-Path $profile.FullName 'deps'
    if (Test-Path -LiteralPath $deps) {
        foreach ($file in Get-ChildItem -LiteralPath $deps -File) {
            $hash = if ($file.Name -match '-([0-9a-f]{16})\.[^.]+$') { $matches[1] } else { $null }
            if (-not $hash -or $live.ContainsKey($hash)) { continue }
            $freed += $file.Length
            $removed++
            if ($Apply) { Remove-Item -LiteralPath $file.FullName -Force }
        }
    }

    $build = Join-Path $profile.FullName 'build'
    if (Test-Path -LiteralPath $build) {
        foreach ($dir in Get-ChildItem -LiteralPath $build -Directory) {
            $hash = ($dir.Name -split '-')[-1]
            if ($hash -match '^[0-9a-f]{16}$' -and -not $live.ContainsKey($hash)) {
                $freed += Get-DirectorySize $dir.FullName
                $removed++
                if ($Apply) { Remove-Item -LiteralPath $dir.FullName -Recurse -Force }
            }
        }
    }

    foreach ($unit in $stale) {
        $freed += Get-DirectorySize $unit.Dir.FullName
        $removed++
        if ($Apply) { Remove-Item -LiteralPath $unit.Dir.FullName -Recurse -Force }
    }

    # Incremental sessions cannot be matched by hash (cargo spells them in
    # another base), and an unused one is only recognisable by age: the units a
    # build reuses were all compiled by a recent build.
    $incremental = Join-Path $profile.FullName 'incremental'
    if (Test-Path -LiteralPath $incremental) {
        $cutoff = (Get-Date).AddDays(-$KeepIncrementalDays)
        foreach ($session in Get-ChildItem -LiteralPath $incremental -Directory) {
            if ($session.LastWriteTime -ge $cutoff) { continue }
            $freed += Get-DirectorySize $session.FullName
            $removed++
            if ($Apply) { Remove-Item -LiteralPath $session.FullName -Recurse -Force }
        }
    }
}

$action = if ($Apply) { 'Removed' } else { 'Would remove' }
Write-Output ('{0} {1} entries, {2:N1} MB. {3}' -f $action, $removed, ($freed / 1MB),
    $(if ($Apply) { 'The next build of the active toolchain reuses everything it needs.' }
      else { 'Run with -Apply to delete.' }))
