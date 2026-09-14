param(
    [string]$Executable = "$PSScriptRoot\..\target\release\g-terminal.exe",
    [ValidateRange(1, 10)][int]$Runs = 3
)
$ErrorActionPreference = 'Stop'
$binary = (Resolve-Path -LiteralPath $Executable).Path
. "$PSScriptRoot\private-working-set.ps1"
# Screenshot mode skips workspace restoration/persistence. Sample before its
# four-second GPU readback, which would itself allocate a screenshot buffer.
for ($run = 1; $run -le $Runs; $run++) {
    $capture = Join-Path ([System.IO.Path]::GetTempPath()) ("gterminal-memory-" + [guid]::NewGuid() + '.png')
    $process = Start-Process -FilePath $binary -ArgumentList "--screenshot `"$capture`"" -WindowStyle Hidden -PassThru
    try {
        for ($sample = 1; $sample -le 4; $sample++) {
            Start-Sleep -Milliseconds 800
            $process.Refresh()
            if ($process.HasExited) { throw 'Application exited before sampling completed.' }
            [pscustomobject]@{
                Run = $run
                Sample = $sample
                WorkingMiB = [math]::Round($process.WorkingSet64 / 1MB, 1)
                PrivateMiB = [math]::Round($process.PrivateMemorySize64 / 1MB, 1)
                PrivateWorkingMiB = [math]::Round([GTerminalMemory]::PrivateWorkingSet($process.Handle) / 1MB, 1)
                Executable = $binary
            }
        }
        if (-not $process.WaitForExit(20000)) { throw 'Screenshot smoke test timed out.' }
        if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $capture)) {
            throw 'Screenshot smoke test failed.'
        }
    } finally {
        if (-not $process.HasExited) { Stop-Process -Id $process.Id }
        $process.Dispose()
        if (Test-Path -LiteralPath $capture) { Remove-Item -LiteralPath $capture }
    }
}
