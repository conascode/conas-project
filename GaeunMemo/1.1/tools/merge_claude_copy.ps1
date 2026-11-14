# GaeunMemo - merge notes that were saved into the Claude app's private
# (virtualized) folder back into the real data folder.
#
# Run this yourself in a normal PowerShell window (NOT from inside Claude),
# after quitting GaeunMemo from the tray menu:
#
#   powershell -ExecutionPolicy Bypass -File merge_claude_copy.ps1
#
# - Backs up the real notes.json first (notes-before-merge-<time>.json).
# - Adds notes that exist only in the Claude copy.
# - For a note present in both, keeps the one edited more recently (updatedAt).
# - Never modifies or deletes the Claude copy.

param(
  [string]$Real = "$env:APPDATA\gaeunmemo\notes.json",
  [string]$Copy = "$env:LOCALAPPDATA\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\gaeunmemo\notes.json",
  [switch]$SkipRunningCheck
)
$ErrorActionPreference = 'Stop'

if (-not $SkipRunningCheck -and (Get-Process gaeunmemo -ErrorAction SilentlyContinue)) {
  Write-Host "GaeunMemo is running. Quit it from the tray menu (last item) and run this again." -ForegroundColor Yellow
  exit 1
}
if (-not (Test-Path $Copy)) { Write-Host "No Claude copy found: $Copy"; exit 0 }

function Read-Data($path) { Get-Content $path -Raw -Encoding UTF8 | ConvertFrom-Json }
function Stamp($n) { if ($n.PSObject.Properties['updatedAt'] -and $n.updatedAt) { [int64]$n.updatedAt } else { [int64]0 } }

$copyData = Read-Data $Copy
if (Test-Path $Real) {
  $realData = Read-Data $Real
  $backup = Join-Path (Split-Path $Real) ("notes-before-merge-{0:yyyyMMdd-HHmmss}.json" -f (Get-Date))
  Copy-Item $Real $backup
  Write-Host "Backup: $backup"
} else {
  New-Item -ItemType Directory -Force (Split-Path $Real) | Out-Null
  $realData = $copyData
  $realData.notes = @()
}

$byId = [ordered]@{}
foreach ($n in @($realData.notes)) { $byId[$n.id] = $n }
$added = 0; $replaced = 0
foreach ($n in @($copyData.notes)) {
  if (-not $byId.Contains($n.id)) { $byId[$n.id] = $n; $added++ }
  elseif ((Stamp $n) -gt (Stamp $byId[$n.id])) { $byId[$n.id] = $n; $replaced++ }
}
$realData.notes = @($byId.Values)

$json = $realData | ConvertTo-Json -Depth 8 -Compress
[IO.File]::WriteAllText($Real, $json, (New-Object Text.UTF8Encoding $false))
Write-Host "Merged into $Real : $added added, $replaced replaced by a newer copy, $($byId.Count) notes total."
Write-Host "Start GaeunMemo again to see them."
