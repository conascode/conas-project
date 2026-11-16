# P0 measurement matrix (report GM2-TDR-001 §6.7 / §13.2).
# Every run is a separate process: off-screen, never activated, no files touched except results.
#   powershell -NoProfile -ExecutionPolicy Bypass -File bench\run_matrix.ps1 [-Reps 3] [-Idle 30]
param([int]$Reps = 3, [int]$Idle = 30, [int]$Type = 10, [int]$LeakCycles = 100)

$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'target\release\gaeunmemo2.exe'
$res = Join-Path $PSScriptRoot 'results'
New-Item -ItemType Directory -Force $res | Out-Null
$runs = Join-Path $res 'matrix.jsonl'
$leaks = Join-Path $res 'leak.jsonl'
foreach ($f in $runs, $leaks) { if (Test-Path $f) { Remove-Item $f } }

$configs = @(
  @{ label = 'tray';  render = 'gdi';      body = 'none';     notes = @(0) },
  @{ label = 'PT-A';  render = 'gdi';      body = 'richedit'; notes = @(1, 10, 50) },
  @{ label = 'PT-B';  render = 'd2d-hwnd'; body = 'richedit'; notes = @(1, 10, 50) },
  @{ label = "PT-B'"; render = 'd2d-dc';   body = 'richedit'; notes = @(1, 10, 50) },
  @{ label = 'PT-C';  render = 'gdi';      body = 'edit';     notes = @(1, 10, 50) }
)

$t0 = Get-Date
for ($r = 1; $r -le $Reps; $r++) {          # reps outermost: drift affects every config alike
  foreach ($c in $configs) {
    foreach ($n in $c.notes) {
      $a = "--mode run --render $($c.render) --body $($c.body) --notes $n --settle 3 --idle $Idle --type $Type --label `"$($c.label)`" --out `"$runs`""
      $p = Start-Process $exe -ArgumentList $a -PassThru -Wait
      "{0:HH:mm:ss} rep{1} {2,-5} n={3,-2} exit={4}" -f (Get-Date), $r, $c.label, $n, $p.ExitCode
    }
  }
}
foreach ($c in $configs | Where-Object { $_.label -ne 'tray' }) {
  $cy = if ($c.render -eq 'd2d-hwnd') { [int]($LeakCycles / 2) } else { $LeakCycles }
  $a = "--mode leak --render $($c.render) --body $($c.body) --batch 50 --cycles $cy --label `"$($c.label)`" --out `"$leaks`""
  $p = Start-Process $exe -ArgumentList $a -PassThru -Wait
  "{0:HH:mm:ss} leak {1,-5} cycles={2} exit={3}" -f (Get-Date), $c.label, $cy, $p.ExitCode
}
"done in {0:N1} min" -f ((Get-Date) - $t0).TotalMinutes
