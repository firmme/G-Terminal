param(
    # Delete for real. Without this the script only reports what it would do.
    [switch]$Apply,
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
    (Get-ChildItem -LiteralPath $path -Recurse -Force -File -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
}

$freed = [long]0
$removed = 0
foreach ($profile in Get-ChildItem -LiteralPath $target -Directory) {
    # A profile directory is the one holding a fingerprint database; anything
    # else (`tmp`) is none of this script's business.
    $fingerprints = Join-Path $profile.FullName '.fingerprint'
    if (-not (Test-Path -LiteralPath $fingerprints)) { continue }

    # The hash in an artifact's name is the metadata hash in the name of the
    # fingerprint directory that still refers to it, so a fingerprint that
    # exists means cargo can reuse the artifact, and one that does not means it
    # never will again.
    $live = @{}
    foreach ($unit in Get-ChildItem -LiteralPath $fingerprints -Directory) {
        $hash = ($unit.Name -split '-')[-1]
        if ($hash -match '^[0-9a-f]{16}$') { $live[$hash] = $true }
    }

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
    $(if ($Apply) { 'The next build reuses everything recent.' } else { 'Run with -Apply to delete.' }))
