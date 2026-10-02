# Resize a window by handle. Used to make a form fully visible for automated clicking,
# which beats wheel-scrolling (wheel events need the pointer over the window and the
# window active; SetWindowPos has no such precondition).
param([string]$Handle, [int]$Width, [int]$Height)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Rz {
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
$h = [IntPtr][int64]$Handle
$r = New-Object Rz+RECT
[Rz]::GetWindowRect($h, [ref]$r) | Out-Null
[Rz]::SetWindowPos($h, [IntPtr]::Zero, $r.Left, $r.Top, $Width, $Height, 0x0004) | Out-Null
Start-Sleep -Milliseconds 500
$r2 = New-Object Rz+RECT
[Rz]::GetWindowRect($h, [ref]$r2) | Out-Null
Write-Output ("resized to {0}x{1}" -f ($r2.Right-$r2.Left), ($r2.Bottom-$r2.Top))
