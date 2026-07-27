<#
.SYNOPSIS
    Runs the QLabs REAPER Music Intelligence MCP self-check.

.DESCRIPTION
    A thin wrapper around "qlabs-reaper-music-mcp doctor". It locates the
    executable, invokes the check, prints a plain-language summary of what a
    failure usually means, and exits with the doctor's own exit code so the
    script can be used in a pipeline.

    doctor inspects configuration, the IPC directory, permissions, the
    installation token, knowledge validation, the knowledge hash, the bridge
    heartbeat and version, the REAPER version when connected, resource-path
    consistency, stale commands, whether the result directory is writable, and
    the server version.

    This script reads and reports. It changes nothing.

.PARAMETER ExecutablePath
    The qlabs-reaper-music-mcp executable. When omitted the script looks in the
    default install location, then in the repository's target\release, then on
    PATH.

.PARAMETER ConfigPath
    The bridge's config.json. When omitted the script looks for the one the
    installer wrote under the REAPER resource path. Without it doctor has no
    installation token and every bridge check is skipped.

.PARAMETER IpcDirectory
    The IPC directory to check. Passed through to doctor when given, and takes
    precedence over the ipc_dir in config.json.

.PARAMETER Json
    Ask doctor for machine-readable JSON output instead of a human report.

.EXAMPLE
    .\run-doctor.ps1

.EXAMPLE
    .\run-doctor.ps1 -Json | ConvertFrom-Json

.EXAMPLE
    .\run-doctor.ps1 -ExecutablePath 'C:\Tools\QLabs\qlabs-reaper-music-mcp.exe'
#>
[CmdletBinding()]
param(
    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $ExecutablePath,

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $ConfigPath,

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $IpcDirectory,

    [Parameter()]
    [switch] $Json
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$ExeName = 'qlabs-reaper-music-mcp.exe'

function Resolve-Executable {
    [CmdletBinding()]
    param([string] $Explicit)

    if ($Explicit) {
        if (-not (Test-Path -LiteralPath $Explicit -PathType Leaf)) {
            throw "No executable at '$Explicit'."
        }
        return (Resolve-Path -LiteralPath $Explicit).ProviderPath
    }

    $candidates = @()
    if ($env:LOCALAPPDATA) {
        $candidates += (Join-Path (Join-Path (Join-Path $env:LOCALAPPDATA 'Programs') 'QLabs-Reaper-MCP') $ExeName)
    }
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $candidates += (Join-Path (Join-Path (Join-Path $repoRoot 'target') 'release') $ExeName)
    $candidates += (Join-Path (Join-Path (Join-Path $repoRoot 'target') 'release') 'qlabs-reaper-music-mcp')

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).ProviderPath
        }
    }

    $onPath = Get-Command 'qlabs-reaper-music-mcp' -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }

    throw ("Could not find $ExeName. Build it with 'cargo build --workspace --release', " +
           "install it with scripts\install.ps1, or pass -ExecutablePath.")
}

function Resolve-ConfigPath {
    <#
        Finds the config.json the installer wrote, so that a bare
        .\run-doctor.ps1 can check the bridge instead of skipping every
        bridge-dependent check. Returns $null when there is nothing to find;
        that is not an error, because the REAPER-free checks still run.
    #>
    [CmdletBinding()]
    param([string] $Explicit)

    if ($Explicit) {
        if (-not (Test-Path -LiteralPath $Explicit -PathType Leaf)) {
            throw "No config.json at '$Explicit'."
        }
        return (Resolve-Path -LiteralPath $Explicit).ProviderPath
    }

    if ($env:QLABS_MCP_CONFIG) { return $null }   # doctor reads it itself

    $resourceRoots = @()
    if ($env:APPDATA)      { $resourceRoots += (Join-Path $env:APPDATA 'REAPER') }
    if ($env:LOCALAPPDATA) { $resourceRoots += (Join-Path $env:LOCALAPPDATA 'REAPER') }

    foreach ($root in $resourceRoots) {
        $candidate = Join-Path (Join-Path (Join-Path $root 'Scripts') 'QLabs-Reaper-MCP') 'config.json'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).ProviderPath
        }
    }

    return $null
}

$exe    = Resolve-Executable -Explicit $ExecutablePath
$config = Resolve-ConfigPath -Explicit $ConfigPath

if (-not $Json) {
    Write-Host ''
    Write-Host '  QLabs REAPER Music Intelligence MCP - doctor' -ForegroundColor White
    Write-Host '  ---------------------------------------------------------------'
    Write-Host "  executable : $exe"
    if ($config)       { Write-Host "  config     : $config" }
    if ($IpcDirectory) { Write-Host "  ipc dir    : $IpcDirectory" }
    if (-not $config -and -not $IpcDirectory -and -not $env:QLABS_MCP_CONFIG -and -not $env:QLABS_MCP_IPC_DIR) {
        Write-Host '  config     : none found; the bridge checks will be skipped'
    }
    Write-Host ''
}

# The doctor subcommand always accepts --json. --ipc-dir is only passed when the
# caller asked for a specific directory; if this build of the CLI does not take
# it there, the call is retried without it rather than reported as a failure.
$baseArgs = @('doctor')
if ($Json) { $baseArgs += '--json' }
if ($config) { $baseArgs += @('--config', $config) }

$argsWithIpc = $baseArgs
if ($IpcDirectory) { $argsWithIpc = $baseArgs + @('--ipc-dir', $IpcDirectory) }

& $exe @argsWithIpc
$exitCode = $LASTEXITCODE

if ($exitCode -ne 0 -and $IpcDirectory) {
    # Distinguish "doctor found problems" from "the CLI rejected the argument"
    # by retrying the plain form once.
    & $exe @baseArgs
    $plainExit = $LASTEXITCODE
    if ($plainExit -ne $exitCode) { $exitCode = $plainExit }
}

if (-not $Json) {
    Write-Host ''
    if ($exitCode -eq 0) {
        Write-Host '  doctor: all checks passed.' -ForegroundColor Green
    } else {
        Write-Host "  doctor: one or more checks failed (exit code $exitCode)." -ForegroundColor Yellow
        Write-Host ''
        Write-Host '  The usual causes, in order of likelihood:'
        Write-Host ''
        Write-Host '   * The bridge is not running. Open REAPER and run the'
        Write-Host '     QLabs_Reaper_MCP_Bridge.lua action. Until you do, the'
        Write-Host '     heartbeat and REAPER-version checks cannot pass.'
        Write-Host '   * The server and the bridge disagree about the IPC'
        Write-Host '     directory. Compare heartbeat.json''s ipc_dir with the'
        Write-Host '     --ipc-dir the server is given.'
        Write-Host '   * The installation token in config.json does not match the'
        Write-Host '     one the server is using. Re-run scripts\install.ps1.'
        Write-Host '   * The IPC directory is not writable by this user account.'
        Write-Host ''
        Write-Host '  Inside REAPER, QLabs_Reaper_MCP_Status.lua reports the same'
        Write-Host '  ground truth from the other side of the boundary.'
    }
    Write-Host ''
}

exit $exitCode
