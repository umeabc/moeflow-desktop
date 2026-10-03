# Click a point given in *window* coordinates — the same space as the images produced by
# capture-window.ps1.
#
# click-launcher-instance.ps1 takes a fraction of the client area, which is fine for "the
# first row" but useless once you need a specific control: the client area is inset from the
# window by the title bar and borders, and the capture is not. Measuring on the capture and
# clicking in the same space removes that conversion entirely.
#
# Real mouse events, deliberately: WebView2 ignores synthetic WM_LBUTTON messages.
param(
  [Parameter(Mandatory=$true)][string]$Title,   # substring of the window title
  [Parameter(Mandatory=$true)][int]$X,
  [Parameter(Mandatory=$true)][int]$Y,
  # The shell windows share a title prefix — "MoeFlow" matches the picker too — so width is
  # what actually distinguishes them.
  [int]$MinWidth = 0,
  [switch]$DoubleClick
)

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class WClicker {
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, System.Text.StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int n);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  public static List<IntPtr> ForPid(uint want) {
    var found = new List<IntPtr>();
    EnumWindows((h,l)=>{ uint p; GetWindowThreadProcessId(h, out p); if (p==want) found.Add(h); return true; }, IntPtr.Zero);
    return found;
  }
  public static string Text(IntPtr h) {
    var sb = new System.Text.StringBuilder(512);
    GetWindowTextW(h, sb, 512);
    return sb.ToString();
  }
}
"@

$proc = Get-Process moeflow-desktop -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Write-Error "moeflow-desktop is not running"; exit 1 }

$target = $null
$rect = New-Object WClicker+RECT
foreach ($h in [WClicker]::ForPid([uint32]$proc.Id)) {
  if (-not [WClicker]::IsWindowVisible($h)) { continue }
  if ([WClicker]::Text($h) -notlike "*$Title*") { continue }
  $r = New-Object WClicker+RECT
  [WClicker]::GetWindowRect($h, [ref]$r) | Out-Null
  if (($r.Right - $r.Left) -lt 100) { continue }
  if (($r.Right - $r.Left) -lt $MinWidth) { continue }
  $target = $h; $rect = $r
}
if (-not $target) { Write-Error "no visible window matching '$Title'"; exit 1 }

# Restore first: a minimised window has a bogus rect, so the coordinates would be wrong.
[WClicker]::ShowWindow($target, 9) | Out-Null
# Raise it too. A background window can swallow the synthetic click entirely — the cursor
# moves, mouse_event fires, and the page never sees it, which is indistinguishable from
# "the control does nothing" unless you check for a side effect.
[WClicker]::SetForegroundWindow($target) | Out-Null
Start-Sleep -Milliseconds 500
[WClicker]::GetWindowRect($target, [ref]$rect) | Out-Null

$sx = $rect.Left + $X
$sy = $rect.Top + $Y
$saved = New-Object WClicker+POINT
[WClicker]::GetCursorPos([ref]$saved) | Out-Null

[WClicker]::SetCursorPos($sx, $sy) | Out-Null
Start-Sleep -Milliseconds 200
[WClicker]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)   # LEFTDOWN
Start-Sleep -Milliseconds 70
[WClicker]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)   # LEFTUP
if ($DoubleClick) {
  Start-Sleep -Milliseconds 60
  [WClicker]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 70
  [WClicker]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
}
Start-Sleep -Milliseconds 500
[WClicker]::SetCursorPos($saved.X, $saved.Y) | Out-Null

Write-Output ("clicked '{0}' window=({1},{2}) screen=({3},{4})" -f [WClicker]::Text($target), $X, $Y, $sx, $sy)
