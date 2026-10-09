[CmdletBinding()]
param()
# Makes Monaka's release zip: dist\monaka_vr-<version>.zip, holding a monaka_vr-<version> folder with
# the window (monaka.exe), monaka_play, the viewer and the loader, the README and the licence. The
# build is the release one of tools\Release.ps1 (C runtime linked in, no paths of this PC inside).
# Upload the zip to a GitHub release of the same version; nothing here publishes anything.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Release.ps1')
$repo = Split-Path -Parent $PSScriptRoot
$version = Get-WorkspaceVersion $repo
$built = Invoke-ReleaseBuild -Workspace $repo -SourceRoot (Split-Path -Parent $repo) -CargoArgs @('-p', 'monaka', '-p', 'monaka_viewer', '-p', 'monaka_loader')

$dist = Join-Path $repo 'dist'
$name = "monaka_vr-$version"
$folder = Join-Path $dist $name
if (Test-Path -LiteralPath $folder) { Remove-Item -LiteralPath $folder -Recurse }
New-Item -ItemType Directory $folder -Force | Out-Null
$programs = 'monaka.exe', 'monaka_play.exe', 'monaka_viewer.exe', 'monaka_loader.exe'
foreach ($program in $programs) { Copy-Item -LiteralPath (Join-Path $built $program) -Destination $folder }
Copy-Item -LiteralPath (Join-Path $repo 'README.md'), (Join-Path $repo 'LICENSE') -Destination $folder
Assert-NoPersonalPaths ($programs | ForEach-Object { Join-Path $folder $_ })

$zip = New-ReleaseZip -Folder $folder -Zip (Join-Path $dist "$name.zip")
Write-Output "Monaka $version`: $zip"
Get-ChildItem -LiteralPath $folder | ForEach-Object { Write-Output ("  {0,-22} {1,10:N0} bytes" -f $_.Name, $_.Length) }
