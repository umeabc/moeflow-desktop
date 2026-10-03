# Move / raise / hide a shell window by size signature.
#
# Needed because screen capture grabs whatever is visually on top: the instance picker sits
# over the settings window at its default position, so a region capture of "the settings
# window" silently returns the picker's pixels instead — and every diff comes back empty,
# which reads as "the click did nothing" when in fact nothing was ever on screen.
param(
  [Parameter(Mandatory=$true)][int]$MinWidth,
  [int]$MaxWidth = 10000,
  [int]$X = -1,
  [int]$Y = -1,
  [switch]$Raise,
  [switch]$Hide,
  [switch]$Show
)

Add-Type @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices;
public class MW {
  public delegate bool E(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(E cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Rt,B; }
  public static List<IntPtr> Find(uint pid, int minW, int maxW) {
    var o = new List<IntPtr>();
    EnumWindows((h,l)=>{ uint p; GetWindowThreadProcessId(h, out p); if (p!=pid) return true;
      R r; GetWindowRect(h, out r); int w = r.Rt-r.L;
      if (w >= minW && w <= maxW) o.Add(h);
      return true; }, IntPtr.Zero);
    return o;
  }
}
'@

$proc = Get-Process moeflow-desktop -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Write-Error "moeflow-desktop is not running"; exit 1 }
$hits = [MW]::Find([uint32]$proc.Id, $MinWidth, $MaxWidth)
if ($hits.Count -eq 0) { Write-Error "no window of width $MinWidth..$MaxWidth"; exit 1 }

foreach ($h in $hits) {
  if ($Hide) { [MW]::ShowWindow($h, 0) | Out-Null }      # SW_HIDE
  if ($Show) { [MW]::ShowWindow($h, 5) | Out-Null }      # SW_SHOW
  if ($X -ge 0 -and $Y -ge 0) {
    # SWP_NOSIZE(0x1) | SWP_NOZORDER(0x4) is deliberately avoided: raising is the point.
    [MW]::SetWindowPos($h, [IntPtr]::Zero, $X, $Y, 0, 0, 0x1) | Out-Null
  }
  if ($Raise) {
    [MW]::ShowWindow($h, 9) | Out-Null                   # SW_RESTORE
    [MW]::SetForegroundWindow($h) | Out-Null
  }
}
$r = New-Object MW+R
[MW]::GetWindowRect($hits[0], [ref]$r) | Out-Null
Write-Output ("window {0}x{1} now at {2},{3}" -f ($r.Rt-$r.L), ($r.B-$r.T), $r.L, $r.T)
