# Builds target\msix\SirenBar_<version>_x64.msix for the Microsoft Store (unsigned: the Store signs it).
#   cargo build --release; powershell -File msix\build.ps1
# Needs makeappx.exe / makepri.exe (NuGet package Microsoft.Windows.SDK.BuildTools); set $env:SDKBIN
# to its bin\<ver>\x64 folder if they are not on PATH.
param([string]$Publisher = '')   # override only for local unsigned test installs

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$ver = ((Get-Content "$root\Cargo.toml" | Select-String '^version = "(.+)"').Matches[0].Groups[1].Value) + '.0'
$sdk = if ($env:SDKBIN) { $env:SDKBIN } else { '' }
$makeappx = Join-Path $sdk 'makeappx.exe'; $makepri = Join-Path $sdk 'makepri.exe'
$out = "$root\target\msix"; $lay = "$out\layout"
Remove-Item -Recurse -Force $lay -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$lay\Assets" | Out-Null

# Manifest
$m = (Get-Content "$PSScriptRoot\AppxManifest.xml" -Raw -Encoding UTF8).Replace('$VERSION$', $ver)
if ($Publisher) { $m = $m -replace 'Publisher="CN=[^"]*"', "Publisher=""$Publisher""" }
[IO.File]::WriteAllText("$lay\AppxManifest.xml", $m, (New-Object Text.UTF8Encoding $false))
Copy-Item "$root\target\release\sirenbar.exe" $lay

# Logos from assets\icon-1024.png at every scale / target size Windows asks for.
Add-Type -AssemblyName System.Drawing
$icon = [Drawing.Image]::FromFile("$root\assets\icon-1024.png")
function Img([int]$w, [int]$h, [int]$s, [string]$name) {
  $b = New-Object Drawing.Bitmap $w, $h
  $g = [Drawing.Graphics]::FromImage($b)
  $g.InterpolationMode = 'HighQualityBicubic'; $g.SmoothingMode = 'AntiAlias'; $g.PixelOffsetMode = 'HighQuality'
  $g.DrawImage($icon, [int](($w - $s) / 2), [int](($h - $s) / 2), $s, $s)
  $g.Dispose(); $b.Save("$lay\Assets\$name", [Drawing.Imaging.ImageFormat]::Png); $b.Dispose()
}
foreach ($k in 100, 125, 150, 200, 400) {
  $f = $k / 100
  $s44 = [int](44 * $f); Img $s44 $s44 $s44 "Square44x44Logo.scale-$k.png"
  $s150 = [int](150 * $f); Img $s150 $s150 ([int]($s150 * 0.66)) "Square150x150Logo.scale-$k.png"
  Img ([int](310 * $f)) ([int](150 * $f)) ([int](150 * $f * 0.66)) "Wide310x150Logo.scale-$k.png"
  $s50 = [int](50 * $f); Img $s50 $s50 $s50 "StoreLogo.scale-$k.png"
}
foreach ($t in 16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256) {
  Img $t $t $t "Square44x44Logo.targetsize-$t.png"
  Img $t $t $t "Square44x44Logo.targetsize-${t}_altform-unplated.png"
  Img $t $t $t "Square44x44Logo.targetsize-${t}_altform-lightunplated.png"
}
$icon.Dispose()

# resources.pri (maps Assets\X.png to the scale / targetsize variants), then the package.
Push-Location $out
& $makepri createconfig /cf priconfig.xml /dq uk-UA /pv 10.0.0 /o | Out-Null
& $makepri new /pr $lay /cf priconfig.xml /mn "$lay\AppxManifest.xml" /of "$lay\resources.pri" /o | Out-Null
Pop-Location
$pkg = "$out\SirenBar_${ver}_x64.msix"
& $makeappx pack /d $lay /p $pkg /o | Out-Null
if ($LASTEXITCODE) { throw "makeappx failed" }
"$pkg  ($([int]((Get-Item $pkg).Length / 1KB)) KB)"
