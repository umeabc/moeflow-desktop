# Click at absolute screen coordinates. Absolute coords avoid the client-rect/DPI math that
# made the fractional helper land unpredictably; the caller derives them from a screenshot.
param([int]$X, [int]$Y, [string]$Keys = "")
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class ClickAt {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
}
"@
Add-Type -AssemblyName System.Windows.Forms
$saved = New-Object ClickAt+POINT
[ClickAt]::GetCursorPos([ref]$saved) | Out-Null
[ClickAt]::SetCursorPos($X, $Y) | Out-Null
Start-Sleep -Milliseconds 200
[ClickAt]::mouse_event(0x0002, 0,0,0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 60
[ClickAt]::mouse_event(0x0004, 0,0,0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 300
if ($Keys) { [System.Windows.Forms.SendKeys]::SendWait($Keys); Start-Sleep -Milliseconds 250 }
[ClickAt]::SetCursorPos($saved.X, $saved.Y) | Out-Null
Write-Output "clicked ($X,$Y) keys='$Keys'"
