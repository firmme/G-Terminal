param(
    [string]$Executable = "$PSScriptRoot\..\target\release\g-terminal.exe",
    [string]$Screenshot = "$PSScriptRoot\..\dist\native-window-smoke.png"
)
$ErrorActionPreference = 'Stop'
if (-not ('GTerminalWindowSmoke' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class GTerminalWindowSmoke {
    private delegate bool Enumerate(IntPtr hwnd, IntPtr param);
    [DllImport("user32.dll")] private static extern bool EnumWindows(Enumerate callback, IntPtr param);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetWindowText(IntPtr hwnd, System.Text.StringBuilder text, int size);
    public static IntPtr FindWindow(int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((hwnd, param) => {
            uint owner; GetWindowThreadProcessId(hwnd, out owner);
            if (owner == pid) {
                var title = new System.Text.StringBuilder(256);
                GetWindowText(hwnd, title, title.Capacity);
                if (title.ToString() == "G-Terminal") { found = hwnd; return false; }
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int mode);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int w, int h, uint flags);
}
'@
}
$binary = (Resolve-Path -LiteralPath $Executable).Path
$capture = [System.IO.Path]::GetFullPath($Screenshot)
$started = [DateTime]::UtcNow
$process = Start-Process -FilePath $binary -ArgumentList "--screenshot `"$capture`"" -WindowStyle Hidden -PassThru
try {
    for ($i=0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 100
        $process.Refresh()
        if ($process.HasExited) { throw 'Application exited during initialization.' }
        $hwnd = [GTerminalWindowSmoke]::FindWindow($process.Id)
        if ($hwnd -ne [IntPtr]::Zero) { break }
    }
    if ($hwnd -eq [IntPtr]::Zero) { throw 'No native window found.' }
    [void][GTerminalWindowSmoke]::SetWindowPos($hwnd, [IntPtr]::Zero, 0, 0, 1000, 650, 0x16)
    Start-Sleep -Milliseconds 300
    [void][GTerminalWindowSmoke]::ShowWindow($hwnd, 6)
    Start-Sleep -Milliseconds 300
    if (-not [GTerminalWindowSmoke]::IsIconic($hwnd)) { throw 'Minimize did not take effect.' }
    [void][GTerminalWindowSmoke]::ShowWindow($hwnd, 9)
    Start-Sleep -Milliseconds 300
    if ([GTerminalWindowSmoke]::IsIconic($hwnd)) { throw 'Restore did not take effect.' }
    if (-not [GTerminalWindowSmoke]::SetWindowPos($hwnd, [IntPtr]::Zero, 0, 0, 1250, 820, 0x16)) { throw 'Resize failed.' }
    Start-Sleep -Milliseconds 200
    if (-not $process.WaitForExit(25000)) { throw 'Resize/minimize/restore screenshot timed out.' }
    if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $capture) -or (Get-Item -LiteralPath $capture).LastWriteTimeUtc -lt $started) { throw 'Native window smoke test failed.' }
    Get-Item -LiteralPath $capture | Select-Object FullName,Length
} finally {
    if (-not $process.HasExited) { Stop-Process -Id $process.Id }
    $process.Dispose()
}
