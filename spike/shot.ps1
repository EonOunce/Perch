param([string]$Out = "D:\Installation\vscode\Perch\spike\shot.png")

# 只用现成程序集，不写内联 C#（PowerShell 5.1 解析 -TypeDefinition 太容易踩坑）。
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$vs = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap($vs.Width, $vs.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($vs.X, $vs.Y, 0, 0, $bmp.Size)
$g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output ("saved {0} ({1}x{2} at {3},{4})" -f $Out, $vs.Width, $vs.Height, $vs.X, $vs.Y)
