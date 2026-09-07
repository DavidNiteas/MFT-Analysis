Add-Type -AssemblyName System.Drawing

# 画一个 32x32 的简单图标并保存为 PNG
$bmp = New-Object System.Drawing.Bitmap 32, 32
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = 'AntiAlias'
$g.Clear([System.Drawing.Color]::FromArgb(30, 31, 36))
$brush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(142, 180, 248))
$g.FillEllipse($brush, 4, 4, 24, 24)
$g.Dispose(); $brush.Dispose()

$ms = New-Object System.IO.MemoryStream
$bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
$png = $ms.ToArray()
$ms.Dispose(); $bmp.Dispose()

# 包一层 ICO 头（ICO 允许内嵌 PNG）
New-Item -ItemType Directory -Force icons | Out-Null
$ico = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ico)
$bw.Write([UInt16]0)          # reserved
$bw.Write([UInt16]1)          # type: icon
$bw.Write([UInt16]1)          # count
$bw.Write([Byte]32)           # width
$bw.Write([Byte]32)           # height
$bw.Write([Byte]0)            # colors
$bw.Write([Byte]0)            # reserved
$bw.Write([UInt16]1)          # planes
$bw.Write([UInt16]32)         # bit count
$bw.Write([UInt32]$png.Length)
$bw.Write([UInt32]22)         # data offset
$bw.Write($png)
$bw.Flush()
[System.IO.File]::WriteAllBytes("$PWD\icons\icon.ico", $ico.ToArray())
$bw.Dispose(); $ico.Dispose()
Write-Output "icon.ico written: $((Get-Item icons\icon.ico).Length) bytes"
