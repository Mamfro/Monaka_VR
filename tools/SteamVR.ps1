# Dot-source this, then call Get-SteamVrOpenXr for SteamVR's OpenXR loader and runtime manifest.
function Get-SteamVrOpenXr {
    $vrPaths=Get-Content -LiteralPath (Join-Path $env:LOCALAPPDATA 'openvr\openvrpaths.vrpath') -Raw | ConvertFrom-Json
    $steamVr=@($vrPaths.runtime)[0]
    $openXr=[pscustomobject]@{ Loader=(Join-Path $steamVr 'bin\win64\openxr_loader.dll'); Runtime=(Join-Path $steamVr 'steamxr_win64.json') }
    foreach ($path in $openXr.Loader,$openXr.Runtime) { if (-not (Test-Path -LiteralPath $path)) { throw "Missing SteamVR component: $path" } }
    $openXr
}
