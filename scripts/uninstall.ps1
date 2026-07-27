<#
.SYNOPSIS
    Removes the QLabs REAPER Music Intelligence MCP from Windows.

.DESCRIPTION
    Removes only files that belong to this product.

    When the install manifest written by install.ps1 is available, exactly the
    files it lists are removed. When it is not, a conservative fallback removes
    only the product's own known filenames. Either way:

      * Unrelated REAPER scripts, presets, themes and configuration are never
        touched. reaper.ini is never read or written.
      * Files you added to the installed knowledge directory - your own
        overrides - are preserved unless you pass -RemoveKnowledgeOverrides.
      * The IPC directory is left in place unless you pass -RemoveIpcDirectory,
        because it is runtime state rather than an installed artefact.
      * Directories are removed only after they are empty.
      * config.json is backed up before it is deleted.

    Nothing is removed while the bridge still looks alive, unless -Force.

.PARAMETER ManifestPath
    An explicit path to install-manifest.json.

.PARAMETER Destination
    The directory the executable was installed to. Defaults to
    $env:LOCALAPPDATA\Programs\QLabs-Reaper-MCP.

.PARAMETER ReaperResourcePath
    The REAPER resource path, when the manifest is unavailable.

.PARAMETER RemoveKnowledgeOverrides
    Also delete files in the installed knowledge directory that the install did
    not put there. Off by default.

.PARAMETER RemoveIpcDirectory
    Also delete the IPC directory and everything in it. Off by default.

.PARAMETER KeepConfig
    Leave config.json in place, so a later reinstall keeps the same
    installation token.

.PARAMETER Force
    Proceed even when the bridge heartbeat suggests the bridge is still running.

.EXAMPLE
    .\uninstall.ps1

.EXAMPLE
    .\uninstall.ps1 -WhatIf

.EXAMPLE
    .\uninstall.ps1 -RemoveKnowledgeOverrides -RemoveIpcDirectory
#>
[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $ManifestPath,

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $Destination,

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $ReaperResourcePath,

    [Parameter()]
    [switch] $RemoveKnowledgeOverrides,

    [Parameter()]
    [switch] $RemoveIpcDirectory,

    [Parameter()]
    [switch] $KeepConfig,

    [Parameter()]
    [switch] $Force
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$ProductName  = 'QLabs REAPER Music Intelligence MCP'
$ScriptFolder = 'QLabs-Reaper-MCP'
$ExeName      = 'qlabs-reaper-music-mcp.exe'
$ManifestName = 'install-manifest.json'

# The conservative fallback list, used only when no manifest is available.
$BridgeTopLevel = @(
    'QLabs_Reaper_MCP_Bridge.lua',
    'QLabs_Reaper_MCP_Status.lua',
    'QLabs_Reaper_MCP_Smoke_Test.lua',
    'README.md'
)
$BridgeLibFiles = @(
    'bridge.lua', 'json.lua', 'protocol.lua', 'snapshot.lua',
    'tagging.lua', 'transactions.lua', 'util.lua'
)

$removed  = 0
$kept     = New-Object System.Collections.ArrayList
$missing  = 0

function Write-Step {
    param([Parameter(Mandatory = $true)][string] $Message)
    Write-Host ''
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Write-Detail {
    param([Parameter(Mandatory = $true)][string] $Message)
    Write-Host "    $Message"
}

function Write-Note {
    param([Parameter(Mandatory = $true)][string] $Message)
    Write-Host "    $Message" -ForegroundColor Yellow
}

function Join-Path3 {
    param(
        [Parameter(Mandatory = $true)][string] $Path,
        [Parameter(Mandatory = $true)][string] $Child1,
        [Parameter(Mandatory = $true)][string] $Child2
    )
    return (Join-Path (Join-Path $Path $Child1) $Child2)
}

function Remove-TrackedFile {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([Parameter(Mandatory = $true)][string] $Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        $script:missing++
        return
    }
    if ($PSCmdlet.ShouldProcess($Path, 'Delete file')) {
        Remove-Item -LiteralPath $Path -Force
        Write-Detail "removed  $Path"
        $script:removed++
    } else {
        Write-Detail "would remove  $Path"
    }
}

function Remove-DirectoryIfEmpty {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([Parameter(Mandatory = $true)][string] $Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return }
    $entries = @(Get-ChildItem -LiteralPath $Path -Force -ErrorAction SilentlyContinue)
    if ($entries.Count -gt 0) {
        [void]$script:kept.Add($Path)
        Write-Note "kept     $Path  (not empty: $($entries.Count) item(s))"
        return
    }
    if ($PSCmdlet.ShouldProcess($Path, 'Remove empty directory')) {
        Remove-Item -LiteralPath $Path -Force
        Write-Detail "removed  $Path"
    } else {
        Write-Detail "would remove  $Path"
    }
}

function Test-BridgeLooksAlive {
    [CmdletBinding()]
    param([string] $IpcDir)

    if (-not $IpcDir) { return $false }
    $heartbeat = Join-Path $IpcDir 'heartbeat.json'
    if (-not (Test-Path -LiteralPath $heartbeat -PathType Leaf)) { return $false }
    try {
        $doc = Get-Content -LiteralPath $heartbeat -Raw -Encoding UTF8 | ConvertFrom-Json
    } catch {
        return $false
    }
    if (-not $doc) { return $false }
    $names = $doc.PSObject.Properties.Name
    if ($names -notcontains 'status' -or $names -notcontains 'timestamp') { return $false }
    if ([string]$doc.status -ne 'online') { return $false }

    $stale = 10
    if ($names -contains 'stale_after_seconds') {
        $candidate = [int]$doc.stale_after_seconds
        if ($candidate -gt 0) { $stale = $candidate }
    }
    $epoch = [DateTime]::SpecifyKind([DateTime]'1970-01-01', [DateTimeKind]::Utc)
    $now   = [int64]([DateTime]::UtcNow - $epoch).TotalSeconds
    $age   = $now - [int64]$doc.timestamp
    return ($age -le $stale)
}

# --------------------------------------------------------------------------
# 1. Work out what was installed
# --------------------------------------------------------------------------

Write-Host ''
Write-Host "  $ProductName - uninstaller" -ForegroundColor White
Write-Host '  ---------------------------------------------------------------'

if (-not $Destination) {
    if ($env:LOCALAPPDATA) {
        $Destination = Join-Path3 $env:LOCALAPPDATA 'Programs' $ScriptFolder
    }
}

if (-not $ManifestPath -and $Destination) {
    $candidate = Join-Path $Destination $ManifestName
    if (Test-Path -LiteralPath $candidate -PathType Leaf) { $ManifestPath = $candidate }
}

$manifest      = $null
$manifestFiles = @()
$scriptDir     = $null
$ipcDir        = $null
$configPath    = $null
$knowledgeDir  = $null

Write-Step 'Locating the installation'

if ($ManifestPath -and (Test-Path -LiteralPath $ManifestPath -PathType Leaf)) {
    try {
        $manifest = Get-Content -LiteralPath $ManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    } catch {
        throw "Install manifest at '$ManifestPath' could not be parsed. Delete it and re-run with -Destination and -ReaperResourcePath."
    }
    if (-not $manifest) {
        throw "Install manifest at '$ManifestPath' is empty. Delete it and re-run with -Destination and -ReaperResourcePath."
    }
    Write-Detail "manifest    $ManifestPath"
    $names = $manifest.PSObject.Properties.Name
    if ($names -contains 'files' -and $manifest.files) { $manifestFiles = @($manifest.files) }
    if ($names -contains 'destination' -and $manifest.destination) { $Destination = [string]$manifest.destination }
    if ($names -contains 'script_dir'  -and $manifest.script_dir)  { $scriptDir   = [string]$manifest.script_dir }
    if ($names -contains 'ipc_dir'     -and $manifest.ipc_dir)     { $ipcDir      = [string]$manifest.ipc_dir }
    if ($names -contains 'config_path' -and $manifest.config_path) { $configPath  = [string]$manifest.config_path }
    Write-Detail "files listed  $($manifestFiles.Count)"
} else {
    Write-Note 'no install manifest found; falling back to the known file list'
    Write-Note 'only this product''s own filenames will be considered'

    if (-not $ReaperResourcePath) {
        if ($env:APPDATA) {
            $candidate = Join-Path $env:APPDATA 'REAPER'
            if (Test-Path -LiteralPath (Join-Path $candidate 'reaper.ini')) {
                $ReaperResourcePath = $candidate
            }
        }
    }
    if (-not $ReaperResourcePath) {
        throw ("Could not locate the REAPER resource path. Re-run with " +
               "-ReaperResourcePath '<Options > Show REAPER resource path>'.")
    }
    $scriptDir  = Join-Path3 $ReaperResourcePath 'Scripts' $ScriptFolder
    $ipcDir     = Join-Path $scriptDir 'ipc'
    $configPath = Join-Path $scriptDir 'config.json'
}

if ($Destination)  { $knowledgeDir = Join-Path $Destination 'knowledge' }

Write-Detail "destination $Destination"
Write-Detail "script dir  $scriptDir"
Write-Detail "ipc dir     $ipcDir"

# --------------------------------------------------------------------------
# 2. Refuse to run while the bridge is live
# --------------------------------------------------------------------------

if (-not $Force -and (Test-BridgeLooksAlive -IpcDir $ipcDir)) {
    throw ("The bridge heartbeat says it is still running. Stop the " +
           "QLabs_Reaper_MCP_Bridge.lua action in REAPER (or close REAPER) and " +
           "re-run. Use -Force to uninstall anyway.")
}

# --------------------------------------------------------------------------
# 3. Remove the installed files
# --------------------------------------------------------------------------

Write-Step 'Removing installed files'

$configIsListed = $false

if ($manifestFiles.Count -gt 0) {
    foreach ($file in $manifestFiles) {
        $path = [string]$file
        if (-not $path) { continue }
        if ($configPath -and ($path -eq $configPath)) { $configIsListed = $true; continue }
        Remove-TrackedFile -Path $path
    }
} else {
    if ($Destination) {
        Remove-TrackedFile -Path (Join-Path $Destination $ExeName)
        Remove-TrackedFile -Path (Join-Path $Destination $ManifestName)
    }
    if ($scriptDir) {
        foreach ($name in $BridgeTopLevel) {
            Remove-TrackedFile -Path (Join-Path $scriptDir $name)
        }
        foreach ($name in $BridgeLibFiles) {
            Remove-TrackedFile -Path (Join-Path3 $scriptDir 'lib' $name)
        }
    }
    if ($knowledgeDir -and (Test-Path -LiteralPath $knowledgeDir -PathType Container)) {
        Write-Note 'without a manifest the installed knowledge files cannot be told'
        Write-Note 'apart from your own overrides, so the knowledge directory is'
        Write-Note 'left alone. Pass -RemoveKnowledgeOverrides to delete all of it.'
    }
}

# --------------------------------------------------------------------------
# 4. Knowledge overrides
# --------------------------------------------------------------------------

if ($knowledgeDir -and (Test-Path -LiteralPath $knowledgeDir -PathType Container)) {
    Write-Step 'Knowledge directory'
    $leftovers = @(Get-ChildItem -LiteralPath $knowledgeDir -Recurse -File -ErrorAction SilentlyContinue)
    if ($leftovers.Count -eq 0) {
        Write-Detail 'nothing left in it'
    } elseif ($RemoveKnowledgeOverrides) {
        foreach ($file in $leftovers) {
            Remove-TrackedFile -Path $file.FullName
        }
    } else {
        Write-Note "$($leftovers.Count) file(s) remain in $knowledgeDir"
        Write-Note 'these were not part of the installation, so they are treated as'
        Write-Note 'your own knowledge overrides and preserved. Pass'
        Write-Note '-RemoveKnowledgeOverrides to delete them too.'
        [void]$kept.Add($knowledgeDir)
    }
}

# --------------------------------------------------------------------------
# 5. Configuration
# --------------------------------------------------------------------------

if ($configPath -and (Test-Path -LiteralPath $configPath -PathType Leaf)) {
    Write-Step 'Configuration'
    if ($KeepConfig) {
        Write-Note "kept     $configPath  (-KeepConfig)"
        [void]$kept.Add($configPath)
    } else {
        $stamp  = (Get-Date).ToString('yyyyMMdd-HHmmss')
        $backup = "$configPath.$stamp.bak"
        if ($PSCmdlet.ShouldProcess($configPath, "Back up to $backup and delete")) {
            Copy-Item -LiteralPath $configPath -Destination $backup -Force
            Write-Note "backed up to  $backup"
            Remove-Item -LiteralPath $configPath -Force
            Write-Detail "removed  $configPath"
            $removed++
            Write-Note 'the backup holds your installation token; delete it too if'
            Write-Note 'you do not intend to reinstall.'
        } else {
            Write-Detail "would back up and remove  $configPath"
        }
    }
} elseif ($configIsListed) {
    Write-Step 'Configuration'
    Write-Detail 'config.json was already gone'
}

# --------------------------------------------------------------------------
# 6. IPC directory
# --------------------------------------------------------------------------

if ($ipcDir -and (Test-Path -LiteralPath $ipcDir -PathType Container)) {
    Write-Step 'IPC directory'
    if ($RemoveIpcDirectory) {
        if ($PSCmdlet.ShouldProcess($ipcDir, 'Delete the IPC directory and its contents')) {
            Remove-Item -LiteralPath $ipcDir -Recurse -Force
            Write-Detail "removed  $ipcDir"
        } else {
            Write-Detail "would remove  $ipcDir"
        }
    } else {
        Write-Note "kept     $ipcDir"
        Write-Note 'it holds runtime state and the bridge log, not installed files.'
        Write-Note 'Pass -RemoveIpcDirectory to delete it.'
        [void]$kept.Add($ipcDir)
    }
}

# --------------------------------------------------------------------------
# 7. Tidy up empty directories
# --------------------------------------------------------------------------

Write-Step 'Removing empty directories'

if ($scriptDir) {
    Remove-DirectoryIfEmpty -Path (Join-Path $scriptDir 'lib')
    Remove-DirectoryIfEmpty -Path $scriptDir
}
if ($knowledgeDir) { Remove-DirectoryIfEmpty -Path $knowledgeDir }
if ($Destination)  { Remove-DirectoryIfEmpty -Path $Destination }

# Never remove <resource>\Scripts itself: other scripts live there.

# --------------------------------------------------------------------------
# 8. Summary
# --------------------------------------------------------------------------

Write-Host ''
Write-Host '  ---------------------------------------------------------------'
if ($WhatIfPreference) {
    Write-Host '  Dry run complete. Nothing was changed.' -ForegroundColor Green
} else {
    Write-Host "  Uninstalled. $removed file(s) removed." -ForegroundColor Green
}
if ($missing -gt 0) {
    Write-Host "  $missing listed file(s) were already gone."
}
if ($kept.Count -gt 0) {
    Write-Host ''
    Write-Host '  Preserved:'
    foreach ($item in $kept) { Write-Host "    $item" }
}
Write-Host ''
Write-Host '  The REAPER action entry itself is not removed by this script.'
Write-Host '  REAPER stores it in its own action list; if you registered the'
Write-Host '  bridge, remove the entry with:'
Write-Host ''
Write-Host '    Actions -> Show action list -> find "QLabs_Reaper_MCP_Bridge"'
Write-Host '    -> Delete'
Write-Host ''
Write-Host '  Remember to remove the "qlabs-reaper-music" entry from your MCP'
Write-Host '  host configuration as well.'
Write-Host ''
