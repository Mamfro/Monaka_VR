[CmdletBinding()]
param([string]$GpuChannel,[switch]$Latest,[switch]$Info,[ValidateRange(1,86400)][int]$Seconds=30,[ValidateRange(0.25,4)][double]$PixelAspect=1)
# Runs the Rust viewer (monaka_viewer) on a live stereo channel, using SteamVR's OpenXR loader.
# -GpuChannel names the channel; -Latest picks the newest run's channel (from the runs folder). -Info prints the
# headset's recommended eye size and exits. For playing, use the Monaka window or monaka_play, which attach the game
# and run this viewer in one go.
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'SteamVR.ps1')
$root=Split-Path -Parent $PSScriptRoot
$viewer=Join-Path $root 'target\release\monaka_viewer.exe'
if (-not (Test-Path -LiteralPath $viewer)) { throw 'Build the viewer first: cargo build --release -p monaka_viewer' }
$openXr=Get-SteamVrOpenXr
if ($Info) { & $viewer --info --loader $openXr.Loader --runtime $openXr.Runtime; exit $LASTEXITCODE }
if ($Latest) {
    $runs=@(Join-Path $root 'runs') | Where-Object { Test-Path -LiteralPath $_ }
    $file=$runs | ForEach-Object { Get-ChildItem -LiteralPath $_ -Directory } | Sort-Object LastWriteTime -Descending |
        ForEach-Object { Join-Path $_.FullName 'live-channel.txt' } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (-not $file) { throw 'No run with a live channel found.' }
    $GpuChannel=(Get-Content -LiteralPath $file -Raw).Trim()
    Write-Output "Channel from $file"
}
if (-not $GpuChannel) { throw 'Give -GpuChannel, -Latest or -Info.' }
& $viewer --gpu-map $GpuChannel --seconds $Seconds --pixel-aspect $PixelAspect.ToString([Globalization.CultureInfo]::InvariantCulture) --loader $openXr.Loader --runtime $openXr.Runtime
exit $LASTEXITCODE
