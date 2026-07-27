<#
.SYNOPSIS
    Builds a release of the QLabs REAPER Music Intelligence MCP and stages a
    distributable directory.

.DESCRIPTION
    Runs a release build, then assembles everything an end user needs into one
    directory:

        <output>\qlabs-reaper-music-mcp-<version>-<platform>\
        |-- qlabs-reaper-music-mcp.exe
        |-- knowledge\                 the theory bundle (also compiled in)
        |-- reaper\                    the Lua bridge, for the REAPER Scripts dir
        |-- scripts\                   install / uninstall / doctor
        |-- docs\                      the full documentation set
        |-- README.md  CHANGELOG.md  LICENSE
        \-- SHA256SUMS.txt             checksums for every staged file

    Optionally compresses the staged directory into a .zip beside it.

    This script only writes inside the output directory. It never modifies the
    repository.

.PARAMETER OutputDirectory
    Where to stage. Defaults to <repo>\dist.

.PARAMETER Version
    Version string for the package name. Defaults to the workspace version read
    from Cargo.toml.

.PARAMETER SourceRoot
    Repository root. Defaults to the parent of this script.

.PARAMETER SkipBuild
    Use the existing target\release build instead of rebuilding.

.PARAMETER Archive
    Also produce a .zip of the staged directory.

.PARAMETER Clean
    Delete an existing staging directory of the same name before staging.

.EXAMPLE
    .\package.ps1

.EXAMPLE
    .\package.ps1 -Archive -Clean

.EXAMPLE
    .\package.ps1 -OutputDirectory 'D:\builds' -WhatIf
#>
[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'Medium')]
param(
    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $OutputDirectory,

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $Version,

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $SourceRoot,

    [Parameter()]
    [switch] $SkipBuild,

    [Parameter()]
    [switch] $Archive,

    [Parameter()]
    [switch] $Clean
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$ExeName = 'qlabs-reaper-music-mcp.exe'
$NixName = 'qlabs-reaper-music-mcp'

function Write-Step {
    param([Parameter(Mandatory = $true)][string] $Message)
    Write-Host ''
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Write-Detail {
    param([Parameter(Mandatory = $true)][string] $Message)
    Write-Host "    $Message"
}

function Get-WorkspaceVersion {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string] $Root)

    $manifest = Join-Path $Root 'Cargo.toml'
    if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) { return 'unknown' }
    foreach ($line in (Get-Content -LiteralPath $manifest)) {
        if ($line -match '^\s*version\s*=\s*"([^"]+)"') { return $Matches[1] }
    }
    return 'unknown'
}

function Copy-Tree {
    <#
        Copies a directory recursively into the staging area.
    #>
    [CmdletBinding(SupportsShouldProcess = $true)]
    param(
        [Parameter(Mandatory = $true)][string] $Source,
        [Parameter(Mandatory = $true)][string] $Target,
        [string[]] $ExcludeDirectory = @()
    )

    if (-not (Test-Path -LiteralPath $Source -PathType Container)) {
        Write-Detail "skipped (absent)  $Source"
        return
    }
    $files = Get-ChildItem -LiteralPath $Source -Recurse -File
    $count = 0
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($Source.Length).TrimStart('\', '/')
        $skip = $false
        foreach ($excluded in $ExcludeDirectory) {
            if ($relative -like "$excluded\*" -or $relative -like "$excluded/*") { $skip = $true; break }
        }
        if ($skip) { continue }
        $destination = Join-Path $Target $relative
        if ($PSCmdlet.ShouldProcess($destination, 'Stage file')) {
            $parent = Split-Path -Parent $destination
            if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
                [void](New-Item -ItemType Directory -Path $parent -Force)
            }
            Copy-Item -LiteralPath $file.FullName -Destination $destination -Force
        }
        $count++
    }
    Write-Detail "staged $count file(s) from $Source"
}

# --------------------------------------------------------------------------
# Resolve inputs
# --------------------------------------------------------------------------

Write-Host ''
Write-Host '  QLabs REAPER Music Intelligence MCP - packager' -ForegroundColor White
Write-Host '  ---------------------------------------------------------------'

if (-not $SourceRoot) { $SourceRoot = Split-Path -Parent $PSScriptRoot }
if (-not (Test-Path -LiteralPath (Join-Path $SourceRoot 'Cargo.toml') -PathType Leaf)) {
    throw "SourceRoot '$SourceRoot' does not contain Cargo.toml. Pass -SourceRoot <repo root>."
}
$SourceRoot = (Resolve-Path -LiteralPath $SourceRoot).ProviderPath

if (-not $Version)         { $Version = Get-WorkspaceVersion -Root $SourceRoot }
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $SourceRoot 'dist' }

$platform = 'windows-x86_64'
if ($env:PROCESSOR_ARCHITECTURE -and $env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
    $platform = 'windows-arm64'
}

$packageName = "qlabs-reaper-music-mcp-$Version-$platform"
$stagingDir  = Join-Path $OutputDirectory $packageName

Write-Detail "repository  $SourceRoot"
Write-Detail "version     $Version"
Write-Detail "package     $packageName"
Write-Detail "staging     $stagingDir"

# --------------------------------------------------------------------------
# Build
# --------------------------------------------------------------------------

$exeSource = Join-Path (Join-Path (Join-Path $SourceRoot 'target') 'release') $ExeName
if (-not (Test-Path -LiteralPath $exeSource -PathType Leaf)) {
    $nixSource = Join-Path (Join-Path (Join-Path $SourceRoot 'target') 'release') $NixName
    if (Test-Path -LiteralPath $nixSource -PathType Leaf) { $exeSource = $nixSource }
}

if ($SkipBuild) {
    Write-Step 'Skipping the build (-SkipBuild)'
    if (-not (Test-Path -LiteralPath $exeSource -PathType Leaf)) {
        throw "No release binary at $exeSource and -SkipBuild was given."
    }
} else {
    Write-Step 'Building the release'
    $cargo = Get-Command 'cargo' -ErrorAction SilentlyContinue
    if (-not $cargo) { throw 'cargo is not on PATH. Install a stable Rust toolchain.' }
    if ($PSCmdlet.ShouldProcess($SourceRoot, 'cargo build --workspace --release')) {
        Push-Location $SourceRoot
        try {
            & cargo build --workspace --release
            if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE." }
        } finally {
            Pop-Location
        }
        if (-not (Test-Path -LiteralPath $exeSource -PathType Leaf)) {
            $nixSource = Join-Path (Join-Path (Join-Path $SourceRoot 'target') 'release') $NixName
            if (Test-Path -LiteralPath $nixSource -PathType Leaf) {
                $exeSource = $nixSource
            } else {
                throw "cargo build succeeded but no binary was found in target\release."
            }
        }
    }
    Write-Detail "binary  $exeSource"
}

# --------------------------------------------------------------------------
# Stage
# --------------------------------------------------------------------------

if ($Clean -and (Test-Path -LiteralPath $stagingDir -PathType Container)) {
    Write-Step 'Cleaning the previous staging directory'
    if ($PSCmdlet.ShouldProcess($stagingDir, 'Delete staging directory')) {
        Remove-Item -LiteralPath $stagingDir -Recurse -Force
        Write-Detail "removed  $stagingDir"
    }
}

Write-Step 'Staging the distributable'

if ($PSCmdlet.ShouldProcess($stagingDir, 'Create staging directory')) {
    [void](New-Item -ItemType Directory -Path $stagingDir -Force)
}

if (Test-Path -LiteralPath $exeSource -PathType Leaf) {
    $exeTarget = Join-Path $stagingDir (Split-Path -Leaf $exeSource)
    if ($PSCmdlet.ShouldProcess($exeTarget, 'Stage the executable')) {
        Copy-Item -LiteralPath $exeSource -Destination $exeTarget -Force
    }
    Write-Detail "staged  $(Split-Path -Leaf $exeSource)"
}

Copy-Tree -Source (Join-Path $SourceRoot 'knowledge') -Target (Join-Path $stagingDir 'knowledge')
Copy-Tree -Source (Join-Path $SourceRoot 'reaper')    -Target (Join-Path $stagingDir 'reaper') -ExcludeDirectory @('tests', 'ipc')
Copy-Tree -Source (Join-Path $SourceRoot 'scripts')   -Target (Join-Path $stagingDir 'scripts')
Copy-Tree -Source (Join-Path $SourceRoot 'docs')      -Target (Join-Path $stagingDir 'docs')
Copy-Tree -Source (Join-Path $SourceRoot 'schemas')   -Target (Join-Path $stagingDir 'schemas')

foreach ($name in @('README.md', 'CHANGELOG.md', 'LICENSE')) {
    $source = Join-Path $SourceRoot $name
    if (Test-Path -LiteralPath $source -PathType Leaf) {
        $target = Join-Path $stagingDir $name
        if ($PSCmdlet.ShouldProcess($target, 'Stage file')) {
            Copy-Item -LiteralPath $source -Destination $target -Force
        }
        Write-Detail "staged  $name"
    }
}

# --------------------------------------------------------------------------
# Checksums
# --------------------------------------------------------------------------

Write-Step 'Writing checksums'

if (Test-Path -LiteralPath $stagingDir -PathType Container) {
    $sumsPath = Join-Path $stagingDir 'SHA256SUMS.txt'
    $lines    = New-Object System.Collections.ArrayList
    $staged   = Get-ChildItem -LiteralPath $stagingDir -Recurse -File | Sort-Object FullName
    foreach ($file in $staged) {
        if ($file.FullName -eq $sumsPath) { continue }
        $hash     = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $relative = $file.FullName.Substring($stagingDir.Length).TrimStart('\', '/').Replace('\', '/')
        [void]$lines.Add("$hash  $relative")
    }
    if ($PSCmdlet.ShouldProcess($sumsPath, 'Write SHA256SUMS.txt')) {
        $encoding = New-Object System.Text.UTF8Encoding($false)
        [System.IO.File]::WriteAllLines($sumsPath, [string[]]$lines.ToArray(), $encoding)
        Write-Detail "wrote $($lines.Count) checksum(s)"
    } else {
        Write-Detail "would write $($lines.Count) checksum(s)"
    }
} else {
    Write-Detail 'nothing staged, so no checksums were written'
}

# --------------------------------------------------------------------------
# Archive
# --------------------------------------------------------------------------

$zipPath = "$stagingDir.zip"
if ($Archive) {
    Write-Step 'Compressing'
    if (Test-Path -LiteralPath $zipPath -PathType Leaf) {
        if ($PSCmdlet.ShouldProcess($zipPath, 'Delete the previous archive')) {
            Remove-Item -LiteralPath $zipPath -Force
        }
    }
    if ($PSCmdlet.ShouldProcess($zipPath, 'Create archive')) {
        Compress-Archive -Path (Join-Path $stagingDir '*') -DestinationPath $zipPath -CompressionLevel Optimal
        Write-Detail "wrote  $zipPath"
    } else {
        Write-Detail "would write  $zipPath"
    }
}

# --------------------------------------------------------------------------
# Summary
# --------------------------------------------------------------------------

Write-Host ''
Write-Host '  ---------------------------------------------------------------'
if ($WhatIfPreference) {
    Write-Host '  Dry run complete. Nothing was written.' -ForegroundColor Green
} else {
    Write-Host '  Packaged.' -ForegroundColor Green
    Write-Host ''
    Write-Host "  Staged   : $stagingDir"
    if ($Archive) { Write-Host "  Archive  : $zipPath" }
    Write-Host ''
    Write-Host '  To verify the package before shipping it:'
    Write-Host ''
    Write-Host '    .\scripts\validate-release.ps1'
    Write-Host ''
    Write-Host '  To install on a target machine, run the installer from inside'
    Write-Host '  the staged directory. It finds the executable beside the'
    Write-Host '  knowledge bundle, so no Rust toolchain is needed there:'
    Write-Host ''
    Write-Host '    cd <staged directory>'
    Write-Host '    .\scripts\install.ps1 -SourceRoot . -SkipBuild'
}
Write-Host ''
