# Read-only window enumeration: which top-level windows exist, are visible, and how big.
# Unlike capture-all-windows.ps1 this never calls ShowWindow, so it observes state without
# changing it — the capture script's own restore would otherwise mask a "did it close?" test.
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class Probe {
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hWnd);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public static List<IntPtr> ForPid(uint want) {
    var found = new List<IntPtr>();
    EnumWindows((h,l)=>{ uint p; GetWindowThreadProcessId(h, out p); if (p==want) found.Add(h); return true; }, IntPtr.Zero);
    return found;
  }
  public static string Title(IntPtr h){ var sb=new StringBuilder(512); GetWindowTextW(h,sb,512); return sb.ToString(); }
}
"@

$mode = $args[0]            # "list" or "close"
$style = if ($args[2]) { $args[2] } else { "close" }   # "close" = WM_CLOSE, "x" = SC_CLOSE
$which = $args[1]           # "launcher" | "main"
$proc = Get-Process moeflow-desktop -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Write-Error "not running"; exit 1 }

function Describe {
  foreach ($h in [Probe]::ForPid([uint32]$proc.Id)) {
    $r = New-Object Probe+RECT
    [Probe]::GetWindowRect($h, [ref]$r) | Out-Null
    $w = $r.Right-$r.Left; $ht = $r.Bottom-$r.Top
    if ($w -lt 400 -or $ht -lt 400) { continue }
    $t = [Probe]::Title($h)
    # Guess the window from its size; the titles are Chinese and do not survive a shell
    # argument on this box. Keep these ranges disjoint from capture-window.ps1's, or a list
    # that says "launcher" can name a window that a capture would not have returned.
    $label = if ($ht -gt 850) { 'main' } elseif ($w -ge 800 -and $w -lt 1100) { 'shell' } else { 'other' }
    Write-Output ("  {0,-9} visible={1,-5} {2}x{3}" -f $label, [Probe]::IsWindowVisible($h), $w, $ht)
  }
}

if ($mode -eq 'list') { Write-Output "windows:"; Describe; exit 0 }

# mode = close
$target = $null
foreach ($h in [Probe]::ForPid([uint32]$proc.Id)) {
  $r = New-Object Probe+RECT
  [Probe]::GetWindowRect($h, [ref]$r) | Out-Null
  $w = $r.Right-$r.Left; $ht = $r.Bottom-$r.Top
  if ($which -eq 'launcher' -and $w -gt 600 -and $w -lt 900 -and $ht -gt 500 -and $ht -lt 800) { $target = $h }
  if ($which -eq 'main'     -and $ht -gt 850 -and $w -gt 1200) { $target = $h }
}
if (-not $target) { Write-Error "no $which window"; exit 1 }
Write-Output "before:"
Describe
if ($style -eq 'x') {
  # What the title-bar X actually sends: WM_SYSCOMMAND with SC_CLOSE.
  [Probe]::PostMessageW($target, 0x0112, [IntPtr]0xF060, [IntPtr]::Zero) | Out-Null
} else {
  [Probe]::PostMessageW($target, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
}
Start-Sleep -Seconds 3
Write-Output "after WM_CLOSE -> $which (hwnd alive: $([Probe]::IsWindow($target)))"
Describe
