# The paths come from the updater as environment variables, never as script text.
$ErrorActionPreference = 'Stop'
$stage = $env:GTERMINAL_UPDATE_DIR
$current = $env:GTERMINAL_UPDATE_CURRENT
$staged = $env:GTERMINAL_UPDATE_STAGED
$parentId = [int]$env:GTERMINAL_UPDATE_PARENT
$backup = [IO.Path]::Combine($stage, 'previous.exe')
$report = [IO.Path]::Combine($stage, 'update-failed.txt')

try {
    try {
        $parent = [Diagnostics.Process]::GetProcessById($parentId)
    } catch [ArgumentException] {
        $parent = $null
    }
    if ($parent) {
        try {
            if (-not $parent.WaitForExit(30000)) {
                throw 'G-Terminal did not exit within 30 seconds.'
            }
        } finally {
            $parent.Dispose()
        }
    }

    [IO.File]::Replace($staged, $current, $backup, $true)
    try {
        $start = New-Object Diagnostics.ProcessStartInfo
        $start.FileName = $current
        $start.WorkingDirectory = [IO.Path]::GetDirectoryName($current)
        $start.UseShellExecute = $false
        foreach ($key in @('PARENT', 'DIR', 'CURRENT', 'STAGED')) {
            $start.EnvironmentVariables.Remove("GTERMINAL_UPDATE_$key")
        }
        $restarted = [Diagnostics.Process]::Start($start)
        $restarted.Dispose()
    } catch {
        try {
            [IO.File]::Replace($backup, $current, $staged, $true)
        } catch {
            throw "Could not restore the previous version. Backup: $backup. $($_.Exception.Message)"
        }
        throw
    }
} catch {
    $message = $_.Exception.Message
    if (-not [IO.File]::Exists($current) -and [IO.File]::Exists($backup)) {
        try {
            [IO.File]::Move($backup, $current)
        } catch {
            $message += "`r`nCould not restore the previous version. Backup: $backup"
        }
    }
    $message = "Could not install the update. Close any other G-Terminal windows before retrying.`r`n`r`n$message"
    try {
        [IO.File]::WriteAllText($report, $message)
    } catch {}
    Add-Type -AssemblyName System.Windows.Forms
    [void][Windows.Forms.MessageBox]::Show("$message`r`n`r`nDetails: $report", 'G-Terminal Update')
    exit 1
}

Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
