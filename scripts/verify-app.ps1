param([int]$WaitSeconds = 20)
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class VFin {
    [DllImport("shcore.dll")] public static extern int SetProcessDpiAwareness(int v);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
    public struct RECT { public int Left, Top, Right, Bottom; }
}
'@
[VFin]::SetProcessDpiAwareness(2) | Out-Null
Start-Sleep -Milliseconds 300
$h = (Get-Process mft-reader | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1).MainWindowHandle
$r = New-Object VFin+RECT
[VFin]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
Write-Output ("rect {0},{1} {2}x{3}" -f $r.Left, $r.Top, $w, $ht)
$topmost = [IntPtr]::new(-1); $notop = [IntPtr]::new(-2)
[VFin]::SetWindowPos($h, $topmost, 0, 0, 0, 0, 0x03) | Out-Null
Start-Sleep -Milliseconds 500
$cx = $r.Left + [int]($w * 0.30)
$cy = $r.Top + [int]($ht * 0.069)
Write-Output ("click {0},{1}" -f $cx, $cy)
[VFin]::SetCursorPos($cx, $cy) | Out-Null
Start-Sleep -Milliseconds 300
[VFin]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 80
[VFin]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Seconds $WaitSeconds
[VFin]::GetWindowRect($h, [ref]$r) | Out-Null
$b = New-Object System.Drawing.Bitmap(($r.Right-$r.Left), ($r.Bottom-$r.Top))
$g = [System.Drawing.Graphics]::FromImage($b)
$g.CopyFromScreen($r.Left, $r.Top, 0, 0, $b.Size)
$b.Save('E:/dev_repos/MFT-reader/shot.png', [System.Drawing.Imaging.ImageFormat]::Png)
[VFin]::SetWindowPos($h, $notop, 0, 0, 0, 0, 0x03) | Out-Null
Write-Output "saved shot.png"
