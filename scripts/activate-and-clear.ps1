# Bring a window to the foreground and clear the focused input.
# SendKeys delivers to the *foreground* window, so a click alone is not enough when another
# process owns the foreground; SetForegroundWindow is attempted after a minimise/restore
# cycle, and the result is reported rather than assumed.
param([string]$Handle, [string]$Keys)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Fg {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
}
"@
Add-Type -AssemblyName System.Windows.Forms

$h = [IntPtr][int64]$Handle
$fg = [Fg]::GetForegroundWindow()
$targetThread = [Fg]::GetWindowThreadProcessId($h, [IntPtr]::Zero)
$myThread = [Fg]::GetCurrentThreadId()
[Fg]::AttachThreadInput($myThread, $targetThread, $true) | Out-Null
[Fg]::ShowWindow($h, 6) | Out-Null   # SW_MINIMIZE
[Fg]::ShowWindow($h, 9) | Out-Null   # SW_RESTORE
[Fg]::BringWindowToTop($h) | Out-Null
[Fg]::SetForegroundWindow($h) | Out-Null
[Fg]::AttachThreadInput($myThread, $targetThread, $false) | Out-Null
Start-Sleep -Milliseconds 800
Write-Output ("foreground acquired: {0}" -f ([Fg]::GetForegroundWindow() -eq $h))
[System.Windows.Forms.SendKeys]::SendWait($Keys)
Start-Sleep -Milliseconds 300
Write-Output "sent: $Keys"
