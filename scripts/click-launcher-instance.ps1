# Drive the launcher the way a person does, to exercise the real click path end to end.
# Moves the real cursor (WebView2 ignores synthetic WM_LBUTTON messages), then puts it back.
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class Clicker {
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int n);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  public static List<IntPtr> ForPid(uint want) {
    var found = new List<IntPtr>();
    EnumWindows((h,l)=>{ uint p; GetWindowThreadProcessId(h, out p); if (p==want) found.Add(h); return true; }, IntPtr.Zero);
    return found;
  }
}
"@

# Fractional position within the launcher client area of the first instance row.
$fx = [double]$args[0]
$fy = [double]$args[1]

$proc = Get-Process moeflow-desktop -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Write-Error "not running"; exit 1 }

$target = $null
foreach ($h in [Clicker]::ForPid([uint32]$proc.Id)) {
  if (-not [Clicker]::IsWindowVisible($h)) { continue }
  $r = New-Object Clicker+RECT
  [Clicker]::GetWindowRect($h, [ref]$r) | Out-Null
  $w = $r.Right-$r.Left; $ht = $r.Bottom-$r.Top
  if ($w -gt 600 -and $w -lt 900 -and $ht -gt 500 -and $ht -lt 800) { $target = $h }
}
if (-not $target) { Write-Error "no launcher window"; exit 1 }

[Clicker]::ShowWindow($target, 9) | Out-Null
[Clicker]::SetForegroundWindow($target) | Out-Null
Start-Sleep -Milliseconds 700

$cr = New-Object Clicker+RECT
[Clicker]::GetClientRect($target, [ref]$cr) | Out-Null
$cw = $cr.Right - $cr.Left; $ch = $cr.Bottom - $cr.Top
$pt = New-Object Clicker+POINT
$pt.X = [int]($cw * $fx); $pt.Y = [int]($ch * $fy)
[Clicker]::ClientToScreen($target, [ref]$pt) | Out-Null

$saved = New-Object Clicker+POINT
[Clicker]::GetCursorPos([ref]$saved) | Out-Null

[Clicker]::SetCursorPos($pt.X, $pt.Y) | Out-Null
Start-Sleep -Milliseconds 250
[Clicker]::mouse_event(0x0002, 0,0,0, [IntPtr]::Zero)   # LEFTDOWN
Start-Sleep -Milliseconds 80
[Clicker]::mouse_event(0x0004, 0,0,0, [IntPtr]::Zero)   # LEFTUP
Start-Sleep -Milliseconds 400
[Clicker]::SetCursorPos($saved.X, $saved.Y) | Out-Null

Write-Output ("clicked launcher: client=({0},{1}) screen=({2},{3})" -f ($pt.X - $cr.Left), ($pt.Y - $cr.Top), $pt.X, $pt.Y)
