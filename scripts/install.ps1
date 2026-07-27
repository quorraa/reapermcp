<#
.SYNOPSIS
    Installs the QLabs REAPER Music Intelligence MCP on Windows.

.DESCRIPTION
    Copies the release executable and the knowledge bundle to a destination
    directory, copies the Lua bridge into the REAPER resource tree, mints a
    cryptographically random installation token, writes the shared config.json
    that both the bridge and the MCP server read, and prints the exact next
    steps including a generic MCP host configuration snippet.

    The script only ever writes files it owns. It creates
    <resource>\Scripts\QLabs-Reaper-MCP\ and writes inside it; it never removes
    or overwrites unrelated REAPER scripts, and it never edits reaper.ini or any
    other REAPER configuration file.

    An install manifest is written to the destination so uninstall.ps1 can
    remove exactly what was installed and nothing else.

.PARAMETER ReaperResourcePath
    The REAPER resource path (Options > Show REAPER resource path). Required for
    portable REAPER installations. When omitted the script looks for the
    standard per-user location.

.PARAMETER Destination
    Where to place the executable and the knowledge bundle. Defaults to
    $env:LOCALAPPDATA\Programs\QLabs-Reaper-MCP. No administrator rights needed.

.PARAMETER IpcDirectory
    An explicit IPC directory. Defaults to <script dir>\ipc, which is what the
    bridge uses when config.json leaves ipc_dir null.

.PARAMETER SourceRoot
    The repository root, or a staged package directory, to install from.
    Defaults to the parent of this script.

.PARAMETER ExecutablePath
    An explicit path to qlabs-reaper-music-mcp.exe. Use this when installing
    from a staged package, where the binary sits beside the knowledge bundle
    rather than under target\release.

.PARAMETER PreserveToken
    Reuse the installation token from an existing config.json instead of minting
    a new one. Useful when reinstalling while an MCP host is already configured.

.PARAMETER SkipBuild
    Do not attempt "cargo build --release" when the executable is missing.

.PARAMETER SkipDoctor
    Do not run the doctor check after installing.

.PARAMETER Force
    Proceed even when the resource path does not look like a REAPER resource
    directory.

.EXAMPLE
    .\install.ps1

.EXAMPLE
    .\install.ps1 -ReaperResourcePath 'D:\REAPER-Portable' -Destination 'C:\Tools\QLabs'

.EXAMPLE
    .\install.ps1 -WhatIf
#>
[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'Medium')]
param(
    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $ReaperResourcePath,

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $Destination,

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $IpcDirectory,

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $SourceRoot,

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $ExecutablePath,

    [Parameter()]
    [switch] $PreserveToken,

    [Parameter()]
    [switch] $SkipBuild,

    [Parameter()]
    [switch] $SkipDoctor,

    [Parameter()]
    [switch] $Force
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

# --------------------------------------------------------------------------
# Constants
# --------------------------------------------------------------------------

$ProductName    = 'QLabs REAPER Music Intelligence MCP'
$ScriptFolder   = 'QLabs-Reaper-MCP'
$ExeName        = 'qlabs-reaper-music-mcp.exe'
$BridgeName     = 'QLabs_Reaper_MCP_Bridge.lua'
$ManifestName   = 'install-manifest.json'
$ManifestFormat = 1

# Files copied into the REAPER script directory. Nothing else is ever touched.
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

# --------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------

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

function New-InstallToken {
    <#
        32 lowercase hex characters from the OS cryptographic RNG. Matches what
        the bridge mints for itself when config.json is missing.
    #>
    $bytes = New-Object 'System.Byte[]' 16
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $rng.GetBytes($bytes)
    } finally {
        if ($rng -is [System.IDisposable]) { $rng.Dispose() }
    }
    $sb = New-Object System.Text.StringBuilder
    foreach ($b in $bytes) { [void]$sb.Append($b.ToString('x2')) }
    return $sb.ToString()
}

function New-DirectoryIfMissing {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([Parameter(Mandatory = $true)][string] $Path)
    if (Test-Path -LiteralPath $Path -PathType Container) { return $false }
    if ($PSCmdlet.ShouldProcess($Path, 'Create directory')) {
        [void](New-Item -ItemType Directory -Path $Path -Force)
        Write-Detail "created  $Path"
        return $true
    }
    Write-Detail "would create  $Path"
    return $false
}

function Copy-Tracked {
    <#
        Copies one file and records it in the install manifest list.
    #>
    [CmdletBinding(SupportsShouldProcess = $true)]
    param(
        [Parameter(Mandatory = $true)][string] $Source,
        [Parameter(Mandatory = $true)][string] $Target,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][System.Collections.ArrayList] $Tracker
    )
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
        throw "Source file not found: $Source"
    }
    if ($PSCmdlet.ShouldProcess($Target, 'Copy file')) {
        $parent = Split-Path -Parent $Target
        if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
            [void](New-Item -ItemType Directory -Path $parent -Force)
        }
        Copy-Item -LiteralPath $Source -Destination $Target -Force
        Write-Detail "copied   $Target"
    } else {
        Write-Detail "would copy  $Target"
    }
    [void]$Tracker.Add($Target)
}

function Backup-File {
    <#
        Backs a file up next to itself with a timestamp suffix. Returns the
        backup path, or $null when there was nothing to back up.
    #>
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([Parameter(Mandatory = $true)][string] $Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    $stamp  = (Get-Date).ToString('yyyyMMdd-HHmmss')
    $backup = "$Path.$stamp.bak"
    if ($PSCmdlet.ShouldProcess($Path, "Back up to $backup")) {
        Copy-Item -LiteralPath $Path -Destination $backup -Force
        Write-Note "backed up existing file to  $backup"
    } else {
        Write-Detail "would back up to  $backup"
    }
    return $backup
}

function Resolve-ReaperResourcePath {
    <#
        Returns the REAPER resource path, or throws with guidance.

        Never assumes %APPDATA%\REAPER is correct: it is only a candidate, and
        it is validated before use. Portable installations must be named
        explicitly with -ReaperResourcePath.
    #>
    param([string] $Explicit, [bool] $AllowUnverified)

    if ($Explicit) {
        if (-not (Test-Path -LiteralPath $Explicit -PathType Container)) {
            throw "The path given to -ReaperResourcePath does not exist: $Explicit"
        }
        $looksRight = (Test-Path -LiteralPath (Join-Path $Explicit 'reaper.ini')) -or
                      (Test-Path -LiteralPath (Join-Path $Explicit 'Scripts') -PathType Container) -or
                      (Test-Path -LiteralPath (Join-Path $Explicit 'Effects') -PathType Container)
        if (-not $looksRight -and -not $AllowUnverified) {
            throw ("'$Explicit' does not look like a REAPER resource directory " +
                   "(no reaper.ini, Scripts or Effects). Re-run with -Force to use it anyway.")
        }
        return (Resolve-Path -LiteralPath $Explicit).ProviderPath
    }

    $candidates = @()
    if ($env:APPDATA)      { $candidates += (Join-Path $env:APPDATA 'REAPER') }
    if ($env:LOCALAPPDATA) { $candidates += (Join-Path $env:LOCALAPPDATA 'REAPER') }

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath (Join-Path $candidate 'reaper.ini')) {
            return (Resolve-Path -LiteralPath $candidate).ProviderPath
        }
    }

    throw ("Could not find a REAPER resource directory automatically. In REAPER, " +
           "choose Options > Show REAPER resource path, then re-run with " +
           "-ReaperResourcePath '<that path>'. Portable installations always need this.")
}

function Get-ExistingToken {
    param([Parameter(Mandatory = $true)][string] $ConfigPath)
    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) { return $null }
    try {
        $doc = Get-Content -LiteralPath $ConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
    } catch {
        Write-Note "existing config.json could not be parsed; a new token will be minted"
        return $null
    }
    if ($doc -and $doc.PSObject.Properties.Name -contains 'instance_token') {
        $token = [string]$doc.instance_token
        if ($token) { return $token }
    }
    return $null
}

function Write-JsonFile {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param(
        [Parameter(Mandatory = $true)][string] $Path,
        [Parameter(Mandatory = $true)] $Value,
        [Parameter(Mandatory = $true)][string] $Action
    )
    $json = $Value | ConvertTo-Json -Depth 8
    if ($PSCmdlet.ShouldProcess($Path, $Action)) {
        # UTF-8 without a BOM: the Lua and Rust readers both expect plain UTF-8.
        $encoding = New-Object System.Text.UTF8Encoding($false)
        [System.IO.File]::WriteAllText($Path, $json, $encoding)
        Write-Detail "wrote    $Path"
    } else {
        Write-Detail "would write  $Path"
    }
}

# --------------------------------------------------------------------------
# 1. Locate the source tree
# --------------------------------------------------------------------------

Write-Host ''
Write-Host "  $ProductName - installer" -ForegroundColor White
Write-Host '  ---------------------------------------------------------------'

if (-not $SourceRoot) { $SourceRoot = Split-Path -Parent $PSScriptRoot }
if (-not (Test-Path -LiteralPath $SourceRoot -PathType Container)) {
    throw "SourceRoot '$SourceRoot' does not exist."
}
$SourceRoot = (Resolve-Path -LiteralPath $SourceRoot).ProviderPath

# Either a repository checkout (Cargo.toml) or a staged package (knowledge +
# reaper beside the binary) is a valid source.
$isRepo   = Test-Path -LiteralPath (Join-Path $SourceRoot 'Cargo.toml') -PathType Leaf
$isStaged = (Test-Path -LiteralPath (Join-Path $SourceRoot 'knowledge') -PathType Container) -and
            (Test-Path -LiteralPath (Join-Path $SourceRoot 'reaper') -PathType Container)
if (-not $isRepo -and -not $isStaged) {
    throw ("SourceRoot '$SourceRoot' is neither a repository checkout (no Cargo.toml) " +
           "nor a staged package (no knowledge\ and reaper\). Pass -SourceRoot <repo root>.")
}

Write-Step 'Locating the source tree'
Write-Detail "repository  $SourceRoot"

if ($ExecutablePath) {
    if (-not (Test-Path -LiteralPath $ExecutablePath -PathType Leaf)) {
        throw "The path given to -ExecutablePath does not exist: $ExecutablePath"
    }
    $exeSource = (Resolve-Path -LiteralPath $ExecutablePath).ProviderPath
} elseif (Test-Path -LiteralPath (Join-Path $SourceRoot $ExeName) -PathType Leaf) {
    # A staged package puts the binary beside the knowledge bundle.
    $exeSource = Join-Path $SourceRoot $ExeName
} else {
    $exeSource = Join-Path (Join-Path3 $SourceRoot 'target' 'release') $ExeName
}
$knowledgeSource = Join-Path $SourceRoot 'knowledge'
$bridgeSource    = Join-Path $SourceRoot 'reaper'

if (-not (Test-Path -LiteralPath $knowledgeSource -PathType Container)) {
    throw "Knowledge bundle not found at $knowledgeSource."
}
if (-not (Test-Path -LiteralPath (Join-Path $bridgeSource $BridgeName) -PathType Leaf)) {
    throw "Lua bridge not found at $(Join-Path $bridgeSource $BridgeName)."
}

# --------------------------------------------------------------------------
# 2. Make sure the release executable exists
# --------------------------------------------------------------------------

Write-Step 'Checking the release executable'

if (-not (Test-Path -LiteralPath $exeSource -PathType Leaf)) {
    if ($SkipBuild) {
        throw "Release executable not found at $exeSource and -SkipBuild was given. Run: cargo build --workspace --release"
    }
    $cargo = Get-Command 'cargo' -ErrorAction SilentlyContinue
    if (-not $cargo) {
        throw ("Release executable not found at $exeSource, and cargo is not on PATH. " +
               "Install a stable Rust toolchain and run: cargo build --workspace --release")
    }
    Write-Detail 'not found; building it now (this can take a few minutes)'
    if ($PSCmdlet.ShouldProcess($SourceRoot, 'cargo build --workspace --release')) {
        Push-Location $SourceRoot
        try {
            & cargo build --workspace --release
            if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE." }
        } finally {
            Pop-Location
        }
        if (-not (Test-Path -LiteralPath $exeSource -PathType Leaf)) {
            throw "cargo build reported success but $exeSource is still missing."
        }
    }
} else {
    Write-Detail "found    $exeSource"
}

# --------------------------------------------------------------------------
# 3. Resolve the destinations
# --------------------------------------------------------------------------

Write-Step 'Resolving destinations'

$resourcePath = Resolve-ReaperResourcePath -Explicit $ReaperResourcePath -AllowUnverified ([bool]$Force)
Write-Detail "REAPER resource path  $resourcePath"

if (-not $Destination) {
    if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is not set; pass -Destination explicitly.' }
    $Destination = Join-Path3 $env:LOCALAPPDATA 'Programs' $ScriptFolder
}
Write-Detail "program destination   $Destination"

$scriptDir = Join-Path3 $resourcePath 'Scripts' $ScriptFolder
$libDir    = Join-Path $scriptDir 'lib'
Write-Detail "bridge destination    $scriptDir"

if (-not $IpcDirectory) { $IpcDirectory = Join-Path $scriptDir 'ipc' }
$ipcIsDefault = ($IpcDirectory -eq (Join-Path $scriptDir 'ipc'))
Write-Detail "ipc directory         $IpcDirectory"

$configPath = Join-Path $scriptDir 'config.json'

# --------------------------------------------------------------------------
# 4. Create directories
# --------------------------------------------------------------------------

Write-Step 'Creating directories'

[void](New-DirectoryIfMissing -Path $Destination)
[void](New-DirectoryIfMissing -Path (Join-Path $Destination 'knowledge'))
[void](New-DirectoryIfMissing -Path $scriptDir)
[void](New-DirectoryIfMissing -Path $libDir)
[void](New-DirectoryIfMissing -Path $IpcDirectory)
# The bridge creates the whole IPC tree at startup, but the server may run
# first, so the two directories it needs are created here as well.
[void](New-DirectoryIfMissing -Path (Join-Path $IpcDirectory 'commands'))
[void](New-DirectoryIfMissing -Path (Join-Path $IpcDirectory 'results'))

# --------------------------------------------------------------------------
# 5. Copy the executable, the knowledge bundle and the Lua bridge
# --------------------------------------------------------------------------

$installedFiles = New-Object System.Collections.ArrayList

Write-Step 'Copying the executable'
$exeTarget = Join-Path $Destination $ExeName
Copy-Tracked -Source $exeSource -Target $exeTarget -Tracker $installedFiles

Write-Step 'Copying the knowledge bundle'
Write-Detail 'the bundle is also compiled into the executable; this copy is for'
Write-Detail 'inspection and for use with --knowledge-dir'
$knowledgeFiles = Get-ChildItem -LiteralPath $knowledgeSource -Recurse -File
foreach ($file in $knowledgeFiles) {
    $relative = $file.FullName.Substring($knowledgeSource.Length).TrimStart('\', '/')
    $target   = Join-Path3 $Destination 'knowledge' $relative
    Copy-Tracked -Source $file.FullName -Target $target -Tracker $installedFiles
}

Write-Step 'Copying the Lua bridge'
Write-Detail 'only this product''s own files are written; unrelated REAPER'
Write-Detail 'scripts in the Scripts directory are never touched'
foreach ($name in $BridgeTopLevel) {
    $src = Join-Path $bridgeSource $name
    if (Test-Path -LiteralPath $src -PathType Leaf) {
        Copy-Tracked -Source $src -Target (Join-Path $scriptDir $name) -Tracker $installedFiles
    }
}
foreach ($name in $BridgeLibFiles) {
    $src = Join-Path3 $bridgeSource 'lib' $name
    Copy-Tracked -Source $src -Target (Join-Path $libDir $name) -Tracker $installedFiles
}

# --------------------------------------------------------------------------
# 6. Installation token and configuration file
# --------------------------------------------------------------------------

Write-Step 'Writing the configuration'

$existingToken = Get-ExistingToken -ConfigPath $configPath
$token = $null
if ($PreserveToken -and $existingToken) {
    $token = $existingToken
    Write-Detail 'reusing the installation token from the existing config.json'
} else {
    $token = New-InstallToken
    if ($existingToken) {
        Write-Note 'a new installation token was minted; any MCP host or bridge'
        Write-Note 'still holding the old one must be restarted'
    }
}

if (Test-Path -LiteralPath $configPath -PathType Leaf) {
    [void](Backup-File -Path $configPath)
}

$ipcDirValue = $null
if (-not $ipcIsDefault) { $ipcDirValue = $IpcDirectory }

$config = [ordered]@{
    instance_token        = $token
    ipc_dir               = $ipcDirValue
    poll_interval_ms      = 50
    heartbeat_interval_ms = 1000
    log_level             = 'info'
    console_log           = $false
}
Write-JsonFile -Path $configPath -Value $config -Action 'Write config.json'
[void]$installedFiles.Add($configPath)

# --------------------------------------------------------------------------
# 7. Install manifest, so the uninstaller removes exactly this and no more
# --------------------------------------------------------------------------

Write-Step 'Writing the install manifest'

$manifest = [ordered]@{
    manifest_format = $ManifestFormat
    product         = $ProductName
    installed_at    = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    source_root     = $SourceRoot
    destination     = $Destination
    resource_path   = $resourcePath
    script_dir      = $scriptDir
    ipc_dir         = $IpcDirectory
    config_path     = $configPath
    files           = @($installedFiles.ToArray())
}
$manifestPath = Join-Path $Destination $ManifestName
Write-JsonFile -Path $manifestPath -Value $manifest -Action 'Write install manifest'

# --------------------------------------------------------------------------
# 8. Next steps
# --------------------------------------------------------------------------

$bridgePath = Join-Path $scriptDir $BridgeName
$exeForJson = $exeTarget.Replace('\', '\\')
$ipcForJson = $IpcDirectory.Replace('\', '\\')

Write-Host ''
Write-Host '  ---------------------------------------------------------------'
Write-Host '  Installed.' -ForegroundColor Green
Write-Host '  ---------------------------------------------------------------'
Write-Host ''
Write-Host '  Executable      : ' -NoNewline; Write-Host $exeTarget
Write-Host '  Knowledge copy  : ' -NoNewline; Write-Host (Join-Path $Destination 'knowledge')
Write-Host '  Lua bridge      : ' -NoNewline; Write-Host $bridgePath
Write-Host '  Configuration   : ' -NoNewline; Write-Host $configPath
Write-Host '  IPC directory   : ' -NoNewline; Write-Host $IpcDirectory
Write-Host ''
Write-Host '  NEXT STEPS' -ForegroundColor White
Write-Host ''
Write-Host '  1) Register the bridge in REAPER.' -ForegroundColor White
Write-Host ''
Write-Host '     REAPER'
Write-Host '     -> Actions'
Write-Host '     -> Show action list'
Write-Host '     -> New action'
Write-Host '     -> Load ReaScript'
Write-Host '     -> Select QLabs_Reaper_MCP_Bridge.lua'
Write-Host '     -> Run the registered action'
Write-Host ''
Write-Host '     The exact file to select is:'
Write-Host ''
Write-Host "       $bridgePath" -ForegroundColor Green
Write-Host ''
Write-Host '     You only register it once. After that it appears in the action'
Write-Host '     list and can be bound to a key or a toolbar button; the script'
Write-Host '     maintains its own toggle state.'
Write-Host ''
Write-Host '  2) THE BRIDGE MUST BE RUNNING.' -ForegroundColor Yellow
Write-Host ''
Write-Host '     Every operation that touches your project - reaper.status,'
Write-Host '     reaper.inspect_selection, reaper.stage_candidate, commit,'
Write-Host '     discard and undo - needs the bridge action to be running inside'
Write-Host '     REAPER. Without it those calls fail with BRIDGE_OFFLINE.'
Write-Host '     Theory search and fixture analysis do not need it.'
Write-Host ''
Write-Host '  3) Add the server to your MCP host.' -ForegroundColor White
Write-Host ''
Write-Host '     This is a normal local stdio MCP server. It opens no port.'
Write-Host '     Configuration file locations differ from host to host - check'
Write-Host '     your client''s documentation for where its server list lives.'
Write-Host '     The configuration itself looks like this:'
Write-Host ''
Write-Host '  {' -ForegroundColor Green
Write-Host '    "mcpServers": {' -ForegroundColor Green
Write-Host '      "qlabs-reaper-music": {' -ForegroundColor Green
Write-Host "        `"command`": `"$exeForJson`"," -ForegroundColor Green
Write-Host '        "args": [' -ForegroundColor Green
Write-Host '          "serve",' -ForegroundColor Green
Write-Host '          "--ipc-dir",' -ForegroundColor Green
Write-Host "          `"$ipcForJson`"" -ForegroundColor Green
Write-Host '        ]' -ForegroundColor Green
Write-Host '      }' -ForegroundColor Green
Write-Host '    }' -ForegroundColor Green
Write-Host '  }' -ForegroundColor Green
Write-Host ''
Write-Host '     Backslashes are escaped because that is JSON, not PowerShell.'
Write-Host '     You can also regenerate this snippet at any time with:'
Write-Host ''
Write-Host "       & '$exeTarget' print-mcp-config"
Write-Host ''
Write-Host '  4) Open REAPER, select one MIDI item or some notes in the MIDI'
Write-Host '     editor, and ask your assistant to check reaper.status.'
Write-Host ''
Write-Host '  Full walkthrough: docs\INSTALL_WINDOWS.md'
Write-Host '  Bridge reference: docs\REAPER_BRIDGE.md'
Write-Host ''

# --------------------------------------------------------------------------
# 9. Doctor
# --------------------------------------------------------------------------

$runDoctor = Join-Path $PSScriptRoot 'run-doctor.ps1'

if ($SkipDoctor) {
    Write-Note 'skipping the doctor check (-SkipDoctor)'
} elseif ($WhatIfPreference) {
    Write-Detail 'would run the doctor check'
} elseif (-not (Test-Path -LiteralPath $exeTarget -PathType Leaf)) {
    Write-Note 'executable not present, so the doctor check was skipped'
} elseif (-not (Test-Path -LiteralPath $runDoctor -PathType Leaf)) {
    Write-Note "run-doctor.ps1 not found beside this script; skipping the check"
} else {
    Write-Step 'Running doctor'
    & $runDoctor -ExecutablePath $exeTarget -IpcDirectory $IpcDirectory
    if ($LASTEXITCODE -ne 0) {
        Write-Host ''
        Write-Note 'doctor reported problems. That is expected before the bridge'
        Write-Note 'has been started for the first time: the heartbeat check'
        Write-Note 'cannot pass until the QLabs_Reaper_MCP_Bridge.lua action is'
        Write-Note 'running inside REAPER. Start it, then re-run:'
        Write-Note '  scripts\run-doctor.ps1'
    }
}

Write-Host ''
