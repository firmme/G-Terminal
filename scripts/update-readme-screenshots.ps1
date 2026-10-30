param(
    [string]$Binary = (Join-Path $PSScriptRoot '../target/release/g-terminal.exe')
)
$ErrorActionPreference = 'Stop'
$binaryPath = (Resolve-Path -LiteralPath $Binary).Path
$outputDirectory = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../docs/screenshots')).Path
$variables = @('GTERMINAL_SCREENSHOT_DEMO', 'GTERMINAL_SCREENSHOT_VIEW', 'GTERMINAL_SCREENSHOT_LIGHT', 'GTERMINAL_SCREENSHOT_DELAY_SECONDS')
$previous = @{}
foreach ($name in $variables) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
try {
    $env:GTERMINAL_SCREENSHOT_DEMO = '1'
    $env:GTERMINAL_SCREENSHOT_LIGHT = '0'
    $env:GTERMINAL_SCREENSHOT_DELAY_SECONDS = '4'
    $captures = [ordered]@{ 'main' = ''; 'menu' = 'menu'; 'serial-connection' = 'serial'; 'tab-activity' = 'activity' }
    foreach ($capture in $captures.GetEnumerator()) {
        $env:GTERMINAL_SCREENSHOT_VIEW = $capture.Value
        # Capture into a temporary file so a failed run cannot leave an old image looking current.
        $temporary = Join-Path $outputDirectory ($capture.Key + '.tmp.png')
        $process = Start-Process -FilePath $binaryPath -ArgumentList "--screenshot `"$temporary`"" -WindowStyle Hidden -PassThru
        if (-not $process.WaitForExit(30000)) {
            $process.Kill()
            throw "Screenshot timed out: $($capture.Key)"
        }
        if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $temporary)) {
            throw "Screenshot failed: $($capture.Key)"
        }
        Move-Item -LiteralPath $temporary -Destination (Join-Path $outputDirectory ($capture.Key + '.png')) -Force
        Write-Output "Updated $($capture.Key).png"
    }
} finally {
    foreach ($name in $variables) { [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process') }
}
