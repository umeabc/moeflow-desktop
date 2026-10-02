# Type text into the foreground app window, optionally scrolling first.
# SendKeys needs the target window focused; SetForegroundWindow is attempted and its result
# is reported because it silently fails when another process owns the foreground.
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Typer {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, int d, IntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
Add-Type -AssemblyName System.Windows.Forms

$launcher = [IntPtr]$args[0]
$text = $args[1]
$wheel = if ($args[2]) { [int]$args[2] } else { 0 }

[Typer]::SetForegroundWindow($launcher) | Out-Null
Start-Sleep -Milliseconds 600
Write-Output ("foreground is target: {0}" -f ([Typer]::GetForegroundWindow() -eq $launcher))

if ($wheel -ne 0) {
  $r = New-Object Typer+RECT
  [Typer]::GetWindowRect($launcher, [ref]$r) | Out-Null
  [Typer]::SetCursorPos([int](($r.Left + $r.Right)/2), [int](($r.Top + $r.Bottom)/2)) | Out-Null
  Start-Sleep -Milliseconds 200
  [Typer]::mouse_event(0x0800, 0, 0, $wheel, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 500
}

if ($text) {
  [System.Windows.Forms.SendKeys]::SendWait($text)
  Start-Sleep -Milliseconds 300
  Write-Output "typed: $text"
}
