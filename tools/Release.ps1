# Dot-source this from a Package.ps1: a release build and the checks every release zip gets.
#
# Invoke-ReleaseBuild builds a workspace for x86_64-pc-windows-msvc into its own target\package
# (the everyday target is left alone) with:
#   - the C runtime linked in (+crt-static), so players need no Visual C++ redistributable;
#   - this PC's folders rewritten out of the binaries (--remap-path-prefix): the source paths in
#     panic messages read monaka_vr\crates\..., not C:\Users\<you>\...;
#   - the debug-file path cut to its file name (/PDBALTPATH).
# With an explicit --target, these flags reach only the shipped code, not build scripts or macros.
# Assert-NoPersonalPaths fails if a file still names the user's profile folder.

function Invoke-ReleaseBuild {
    param([Parameter(Mandatory)][string]$Workspace, [Parameter(Mandatory)][string]$SourceRoot, [string[]]$CargoArgs = @())
    $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $root = $SourceRoot.TrimEnd('\') + '\'
    # The last matching prefix wins: the general one first, the specific ones after.
    $flags = @('-C', 'target-feature=+crt-static', '-C', 'link-arg=/PDBALTPATH:%_PDB%')
    foreach ($pair in @(@(($env:USERPROFILE.TrimEnd('\') + '\'), 'user\'), @(($cargoHome.TrimEnd('\') + '\'), 'cargo\'), @($root, ''))) {
        $flags += "--remap-path-prefix=$($pair[0])=$($pair[1])"
        $flags += "--remap-path-prefix=$($pair[0].Replace('\','/'))=$($pair[1].Replace('\','/'))"
    }
    $target = Join-Path $Workspace 'target\package'
    $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
    try {
        Push-Location $Workspace
        & cargo build --release --target x86_64-pc-windows-msvc --target-dir $target @CargoArgs
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed in $Workspace" }
    } finally {
        Pop-Location
        Remove-Item Env:\CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
    }
    Join-Path $target 'x86_64-pc-windows-msvc\release'
}

function Assert-NoPersonalPaths {
    param([Parameter(Mandatory)][string[]]$Files)
    $userName = Split-Path -Leaf $env:USERPROFILE
    $needles = @($env:USERPROFILE, $env:USERPROFILE.Replace('\','/'), "Users\$userName", "Users/$userName")
    foreach ($file in $Files) {
        $bytes = [IO.File]::ReadAllBytes($file)
        foreach ($text in @([Text.Encoding]::ASCII.GetString($bytes), [Text.Encoding]::Unicode.GetString($bytes))) {
            foreach ($needle in $needles) {
                if ($text.IndexOf($needle, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
                    throw "$file still contains '$needle'; it must not ship."
                }
            }
        }
    }
}

# The workspace version, from [workspace.package] in Cargo.toml.
function Get-WorkspaceVersion {
    param([Parameter(Mandatory)][string]$Workspace)
    $line = Select-String -LiteralPath (Join-Path $Workspace 'Cargo.toml') -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if (-not $line) { throw "No version in $Workspace\Cargo.toml" }
    $line.Matches[0].Groups[1].Value
}

# A zip of $Folder (the folder itself at the zip's top) at $Zip, replacing an older one.
function New-ReleaseZip {
    param([Parameter(Mandatory)][string]$Folder, [Parameter(Mandatory)][string]$Zip)
    if (Test-Path -LiteralPath $Zip) { Remove-Item -LiteralPath $Zip }
    Compress-Archive -Path $Folder -DestinationPath $Zip
    $Zip
}
