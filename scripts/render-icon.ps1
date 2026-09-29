<#
.SYNOPSIS
  Renders the FFMincer icon files from the SVG sources in assets/icon: PNGs and ffmincer.ico.
.DESCRIPTION
  Sources: ffmincer.svg (the full icon, 64 px and up) and ffmincer-small.svg (three pieces of the trail instead of four, for
  up to 48 px). Needs resvg on PATH (scoop install resvg). Running it again without
  changes to the SVGs gives the same files.
#>
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$dir = Join-Path (Split-Path $PSScriptRoot -Parent) 'assets\icon'

function Render([string]$svg, [int]$size, [string]$png) {
    & resvg -w $size -h $size (Join-Path $dir $svg) (Join-Path $dir $png)
    if ($LASTEXITCODE -ne 0) { throw "resvg failed for $svg at $size px" }
}

foreach ($s in 16, 24, 32, 48) { Render 'ffmincer-small.svg' $s "ffmincer-$s.png" }
foreach ($s in 64, 128, 256, 512) { Render 'ffmincer.svg' $s "ffmincer-$s.png" }

# ffmincer.ico: PNG frames (Windows Vista and later), 16 to 256.
$sizes = 16, 24, 32, 48, 64, 128, 256
$ms = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($ms)
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
$blobs = @()
foreach ($s in $sizes) {
    $b = [System.IO.File]::ReadAllBytes((Join-Path $dir "ffmincer-$s.png"))
    $blobs += , $b
    # Width and height bytes: 0 means 256.
    $w.Write([byte]($s % 256)); $w.Write([byte]($s % 256)); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$b.Length); $w.Write([uint32]$offset)
    $offset += $b.Length
}
foreach ($b in $blobs) { $w.Write($b) }
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $dir 'ffmincer.ico'), $ms.ToArray())
Write-Host "Icon files written to $dir"
