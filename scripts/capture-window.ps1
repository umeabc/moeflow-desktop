# Capture ONE window of the app by handle, using PrintWindow so the result does not depend
# on the window being in the foreground — SetForegroundWindow silently fails when the
# calling process is not already the foreground process, which is why capturing on a busy
# desktop otherwise photographs whatever happens to be on top instead.
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class WinCap {
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public static List<IntPtr> ForPid(uint want) {
    var found = new List<IntPtr>();
    EnumWindows((h,l)=>{ uint p; GetWindowThreadProcessId(h, out p); if (p==want) found.Add(h); return true; }, IntPtr.Zero);
    return found;
  }
}
"@

# $args[0] = "main" | "launcher" | "settings", $args[1] = output path
$which = $args[0]
$out = $args[1]

$proc = Get-Process moeflow-desktop -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Write-Error "moeflow-desktop not running"; exit 1 }

$target = $null
foreach ($h in [WinCap]::ForPid([uint32]$proc.Id)) {
  $r = New-Object WinCap+RECT
  [WinCap]::GetWindowRect($h, [ref]$r) | Out-Null
  $w = $r.Right-$r.Left; $ht = $r.Bottom-$r.Top
  if ($w -lt 400 -or $ht -lt 400) { continue }
  # The ranges must not overlap. They used to: "launcher" matched 600-900 wide and
  # "settings" 700-1000, so with both open the settings window satisfied *both* tests and
  # whichever ran last won — asking for the launcher handed back the settings window.
  # Windows are told apart by size because their titles are Chinese, which does not survive
  # the round trip through a shell argument on this box.
  if ($which -eq 'main'     -and $ht -gt 850 -and $w -gt 1200) { $target = $h }
  if ($which -eq 'launcher' -and $w -gt 600 -and $w -lt 815 -and $ht -gt 500 -and $ht -lt 800) { $target = $h }
  if ($which -eq 'settings' -and $w -ge 815 -and $w -lt 1000 -and $ht -gt 550 -and $ht -lt 800) { $target = $h }
}
if (-not $target) { Write-Error "no '$which' window found"; exit 1 }

$r = New-Object WinCap+RECT
[WinCap]::GetWindowRect($target, [ref]$r) | Out-Null
$w = $r.Right-$r.Left; $ht = $r.Bottom-$r.Top

$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
# PW_RENDERFULLCONTENT (2) is what makes this work for WebView2 / DirectComposition content.
$ok = [WinCap]::PrintWindow($target, $hdc, 2)
$g.ReleaseHdc($hdc)
if (-not $ok) { Write-Warning "PrintWindow returned false; image may be blank" }
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output ("captured {0} ({1}x{2}) -> {3}" -f $which, $w, $ht, $out)
