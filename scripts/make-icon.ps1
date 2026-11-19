# MediaTranscriber 아이콘 생성 스크립트
#
# 출력 (assets/):
#   icon.ico        - 실행 파일·설치 프로그램 아이콘 (16~256px, PNG 항목)
#   icon-256.png    - README 등 문서용
#   icon-64.rgba    - 창 아이콘용 원시 RGBA (64x64, PNG 디코더 의존성 없이 include_bytes! 로 사용)
#
# 사용: powershell -ExecutionPolicy Bypass -File scripts\make-icon.ps1

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
$assets = Join-Path $root "assets"
New-Item -ItemType Directory -Force $assets | Out-Null

function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)

    $s = $size / 256.0
    # 둥근 사각형 배경 (짙은 남색 그라데이션)
    $r = 56 * $s
    $rect = New-Object System.Drawing.RectangleF (8 * $s), (8 * $s), (240 * $s), (240 * $s)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $r * 2
    $path.AddArc($rect.X, $rect.Y, $d, $d, 180, 90)
    $path.AddArc($rect.Right - $d, $rect.Y, $d, $d, 270, 90)
    $path.AddArc($rect.Right - $d, $rect.Bottom - $d, $d, $d, 0, 90)
    $path.AddArc($rect.X, $rect.Bottom - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $bg = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, `
        ([System.Drawing.Color]::FromArgb(255, 32, 52, 86)), `
        ([System.Drawing.Color]::FromArgb(255, 18, 26, 44)), 90.0
    $g.FillPath($bg, $path)

    # 왼쪽: 음성 파형 막대
    $accent = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 79, 195, 247))
    $heights = @(44, 92, 136, 92, 60)
    $barW = 16 * $s
    $gap = 10 * $s
    $x = 40 * $s
    $cy = 128 * $s
    foreach ($h in $heights) {
        $hh = $h * $s
        $g.FillRectangle($accent, $x, $cy - $hh / 2, $barW, $hh)
        $x += $barW + $gap
    }

    # 오른쪽: 텍스트 줄 (변환 결과)
    $white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 236, 240, 245))
    $lineH = 16 * $s
    $lx = 172 * $s
    foreach ($row in @(@(84, 52), @(120, 44), @(156, 52))) {
        $g.FillRectangle($white, $lx, $row[0] * $s, $row[1] * $s, $lineH)
    }
    # 화살표 (파형 → 텍스트)
    $arrow = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 236, 240, 245)), (10 * $s)
    $arrow.EndCap = [System.Drawing.Drawing2D.LineCap]::ArrowAnchor
    if ($size -ge 32) { $g.DrawLine($arrow, 172 * $s, 210 * $s, 220 * $s, 210 * $s) }

    $g.Dispose()
    return $bmp
}

function Get-PngBytes($bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $ms.ToArray()   # 쉼표: byte[] 가 파이프라인에서 풀리지 않게
}

# ICO (PNG 항목) 작성
$sizes = @(16, 24, 32, 48, 64, 128, 256)
$pngs = @()
foreach ($sz in $sizes) {
    $b = New-IconBitmap $sz
    $pngs += , (Get-PngBytes $b)
    if ($sz -eq 256) { [System.IO.File]::WriteAllBytes((Join-Path $assets "icon-256.png"), (Get-PngBytes $b)) }
    if ($sz -eq 64) {
        # BGRA → RGBA 원시 바이트
        $data = $b.LockBits((New-Object System.Drawing.Rectangle 0, 0, 64, 64), `
            [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $raw = New-Object byte[] (64 * 64 * 4)
        for ($y = 0; $y -lt 64; $y++) {
            [System.Runtime.InteropServices.Marshal]::Copy([IntPtr]($data.Scan0.ToInt64() + $y * $data.Stride), $raw, $y * 256, 256)
        }
        $b.UnlockBits($data)
        for ($i = 0; $i -lt $raw.Length; $i += 4) { $t = $raw[$i]; $raw[$i] = $raw[$i + 2]; $raw[$i + 2] = $t }
        [System.IO.File]::WriteAllBytes((Join-Path $assets "icon-64.rgba"), $raw)
    }
    $b.Dispose()
}

$ms = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $ms
$w.Write([UInt16]0); $w.Write([UInt16]1); $w.Write([UInt16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $sz = $sizes[$i]; $len = $pngs[$i].Length
    $dim = if ($sz -ge 256) { 0 } else { $sz }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([UInt16]1); $w.Write([UInt16]32); $w.Write([UInt32]$len); $w.Write([UInt32]$offset)
    $offset += $len
}
foreach ($p in $pngs) { $w.Write($p) }
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $assets "icon.ico"), $ms.ToArray())
Write-Output "icon.ico / icon-256.png / icon-64.rgba 생성 완료: $assets"
