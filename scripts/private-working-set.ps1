# Query resident pages instead of confusing PrivateMemorySize64 (commit) with
# Task Manager's private working set. This only reads the target's page list.
if (-not ('GTerminalMemory' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class GTerminalMemory {
    [DllImport("psapi.dll", SetLastError=true)]
    private static extern bool QueryWorkingSet(IntPtr process, IntPtr buffer, int size);
    public static long PrivateWorkingSet(IntPtr process) {
        int size = 1024 * 1024;
        while (size <= 32 * 1024 * 1024) {
            IntPtr buffer = Marshal.AllocHGlobal(size);
            try {
                if (QueryWorkingSet(process, buffer, size)) {
                    long count = Marshal.ReadIntPtr(buffer).ToInt64();
                    long pages = 0;
                    for (long i = 0; i < count; i++) {
                        long flags = Marshal.ReadIntPtr(buffer, checked((int)((i+1)*IntPtr.Size))).ToInt64();
                        if ((flags & 0x100) == 0) pages++;
                    }
                    return pages * Environment.SystemPageSize;
                }
                if (Marshal.GetLastWin32Error() != 24) throw new System.ComponentModel.Win32Exception();
            } finally { Marshal.FreeHGlobal(buffer); }
            size *= 2;
        }
        throw new InvalidOperationException("Working set query exceeds limit.");
    }
}
'@
}
