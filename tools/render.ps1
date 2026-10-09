Add-Type -AssemblyName System.Drawing
$d = Get-Content preview_points.json -Raw -Encoding UTF8 | ConvertFrom-Json
function Render($path, $sx, $sy, $sw, $sh, $outW) {
  $k = $outW / $sw; $outH = [int]($sh * $k)
  $bmp = New-Object System.Drawing.Bitmap $outW, $outH
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = 'AntiAlias'; $g.Clear([System.Drawing.Color]::White)
  $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(60,60,60)), 1
  $pen.LineJoin = 'Round'
  $i = 0
  foreach ($r in $d.regions) {
    $c = [System.Drawing.Color]::FromArgb(255, (150 + (($i*47) % 100)), (150 + (($i*83) % 100)), (150 + (($i*29) % 100)))
    $br = New-Object System.Drawing.SolidBrush $c
    foreach ($ring in $r.rings) {
      $pts = New-Object 'System.Drawing.PointF[]' $ring.Count
      for ($j=0; $j -lt $ring.Count; $j++) { $pts[$j] = New-Object System.Drawing.PointF ((($ring[$j][0]-$sx)*$k), (($ring[$j][1]-$sy)*$k)) }
      $g.FillPolygon($br, $pts); $g.DrawPolygon($pen, $pts)
    }
    $i++
  }
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $bmp.Dispose()
}
Render (Join-Path $PWD 'preview_full.png') 0 0 $d.w $d.h 1200
# Crimea / Kherson / Zaporizhzhia coast crop (view units)
Render (Join-Path $PWD 'preview_crimea.png') 1900 1750 1600 900 1600
