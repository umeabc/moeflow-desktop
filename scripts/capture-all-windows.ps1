# Enumerate every top-level window owned by moeflow-desktop and capture each to a PNG.
# Used to diagnose windows that render blank — a hidden/blank window is invisible to
# the main-window-only capture, which silently returns whichever window Windows picks.
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class WinEnum {
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int n);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }

  public static List<IntPtr> ForPid(uint want) {
    var found = new List<IntPtr>();
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == want) found.Add(h); return true; }, IntPtr.Zero);
    return found;
  }
  public static string Title(IntPtr h) { var sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
}
"@

$proc = Get-Process moeflow-desktop -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Write-Error "moeflow-desktop not running"; exit 1 }

$dir = $args[0]
if (-not $dir) { $dir = "C:\Users\Administrator\AppData\Local\Temp\mfd-windows" }
New-Item -ItemType Directory -Force -Path $dir | Out-Null

$i = 0
foreach ($h in [WinEnum]::ForPid([uint32]$proc.Id)) {
  $title = [WinEnum]::Title($h)
  $vis = [WinEnum]::IsWindowVisible($h)
  $r = New-Object WinEnum+RECT
  [WinEnum]::GetWindowRect($h, [ref]$r) | Out-Null
  $w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
  Write-Output ("[{0}] visible={1} rect={2},{3} {4}x{5} title='{6}'" -f $i, $vis, $r.Left, $r.Top, $w, $ht, $title)

  if ($w -gt 50 -and $ht -gt 50) {
    [WinEnum]::ShowWindow($h, 9) | Out-Null
    [WinEnum]::SetForegroundWindow($h) | Out-Null
    Start-Sleep -Milliseconds 1200
    $bmp = New-Object System.Drawing.Bitmap $w, $ht
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
    $out = Join-Path $dir ("window-{0}.png" -f $i)
    $bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Write-Output ("      saved -> {0}" -f $out)
  }
  $i++
}
