param(
    [string]$Executable = "$PSScriptRoot\..\target\release\g-terminal.exe",
    [ValidateRange(10, 3600)][int]$DurationSeconds = 60,
    [string]$Csv = ''
)
$ErrorActionPreference = 'Stop'
$binary = (Resolve-Path -LiteralPath $Executable).Path
$capture = Join-Path ([System.IO.Path]::GetTempPath()) ("gterminal-idle-" + [guid]::NewGuid() + '.png')
$oldDelay = $env:GTERMINAL_SCREENSHOT_DELAY_SECONDS
$probe = $null
$samples = [System.Collections.Generic.List[object]]::new()
try {
    # Screenshot mode starts a separate shell and never restores/saves the user's
    # workspace or performs cloud sync. Delay capture to measure steady idle.
    $env:GTERMINAL_SCREENSHOT_DELAY_SECONDS = [string]($DurationSeconds + 3)
    $probe = Start-Process -FilePath $binary -ArgumentList "--screenshot `"$capture`"" -WindowStyle Hidden -PassThru
    Start-Sleep -Seconds 2
    $probe.Refresh()
    if ($probe.HasExited) { throw 'Application exited during startup.' }
    $lastCpu = $probe.TotalProcessorTime.TotalSeconds
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $lastTime = $clock.Elapsed.TotalSeconds
    while ($clock.Elapsed.TotalSeconds -lt $DurationSeconds) {
        Start-Sleep -Seconds 2
        $probe.Refresh()
        if ($probe.HasExited) { break }
        $now = $clock.Elapsed.TotalSeconds
        $cpu = $probe.TotalProcessorTime.TotalSeconds
        $sample = [pscustomobject]@{
            Seconds = [math]::Round($now, 1)
            CpuPercent = [math]::Round(100 * ($cpu - $lastCpu) / ($now - $lastTime) / [Environment]::ProcessorCount, 2)
            PrivateMiB = [math]::Round($probe.PrivateMemorySize64 / 1MB, 2)
            WorkingMiB = [math]::Round($probe.WorkingSet64 / 1MB, 2)
            Handles = $probe.HandleCount
        }
        $samples.Add($sample)
        $sample
        $lastCpu = $cpu
        $lastTime = $now
    }
    if (-not $probe.WaitForExit(20000)) { throw 'Idle probe did not exit after capture.' }
    if ($probe.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $capture)) {
        throw 'Idle probe failed to render/capture.'
    }
    if ($Csv) { $samples | Export-Csv -LiteralPath $Csv -NoTypeInformation -Encoding utf8 }
} finally {
    $env:GTERMINAL_SCREENSHOT_DELAY_SECONDS = $oldDelay
    if ($probe) {
        if (-not $probe.HasExited) { Stop-Process -Id $probe.Id }
        $probe.Dispose()
    }
    if (Test-Path -LiteralPath $capture) { Remove-Item -LiteralPath $capture }
}
