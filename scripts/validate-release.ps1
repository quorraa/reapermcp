<#
.SYNOPSIS
    Runs every quality gate for the QLabs REAPER Music Intelligence MCP and
    prints a pass/fail summary.

.DESCRIPTION
    A thin wrapper around the checks that must pass before a release is
    shipped. Each gate is run in order, its result is reported, and the script
    exits non-zero if any of them failed.

    Gates:

      1  cargo fmt --all --check                  formatting
      2  cargo clippy --workspace --all-targets   lints, warnings denied
      3  cargo test --workspace                   the Rust test suite
      4  cargo run -p xtask -- check-all          knowledge, schemas, fixtures
      5  lua5.4 tests/run_tests.lua               the REAPER bridge suite
      6  cargo build --workspace --release        the release build
      7  qlabs-reaper-music-mcp version           the binary starts
      8  qlabs-reaper-music-mcp validate-knowledge  the embedded bundle

    Every gate runs offline. The workspace has no external dependencies.

    Gate 5 is skipped with a clear message when lua5.4 is not installed; that
    is a skip, not a pass. Gate 7 and 8 are skipped if the release build was
    skipped.

    This script reads and builds. It does not install anything, does not touch
    REAPER, and does not need the bridge to be running.

    NOTE: this does not and cannot run the in-REAPER smoke test. That one is
    manual and needs a real REAPER host. See docs\TESTING.md.

.PARAMETER SourceRoot
    Repository root. Defaults to the parent of this script.

.PARAMETER SkipRelease
    Skip the release build (gate 6) and the gates that depend on it.

.PARAMETER SkipLua
    Skip the REAPER bridge test suite (gate 5).

.PARAMETER StopOnFirstFailure
    Stop as soon as a gate fails instead of running the rest.

.EXAMPLE
    .\validate-release.ps1

.EXAMPLE
    .\validate-release.ps1 -SkipRelease -StopOnFirstFailure
#>
[CmdletBinding()]
param(
    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string] $SourceRoot,

    [Parameter()]
    [switch] $SkipRelease,

    [Parameter()]
    [switch] $SkipLua,

    [Parameter()]
    [switch] $StopOnFirstFailure
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Continue'

$ExeName = 'qlabs-reaper-music-mcp.exe'
$NixName = 'qlabs-reaper-music-mcp'

$results = New-Object System.Collections.ArrayList
$failed  = 0

function Add-Result {
    param(
        [Parameter(Mandatory = $true)][string] $Name,
        [Parameter(Mandatory = $true)][string] $Status,
        [string] $Note = ''
    )
    [void]$script:results.Add([PSCustomObject]@{
        Name   = $Name
        Status = $Status
        Note   = $Note
    })
}

function Invoke-Gate {
    <#
        Runs one gate in a working directory and records the outcome.
        Returns $true when the gate passed.
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string] $Name,
        [Parameter(Mandatory = $true)][string] $WorkingDirectory,
        [Parameter(Mandatory = $true)][string] $Command,
        [Parameter(Mandatory = $true)][string[]] $Arguments
    )

    Write-Host ''
    Write-Host "==> $Name" -ForegroundColor Cyan
    Write-Host "    $Command $($Arguments -join ' ')" -ForegroundColor DarkGray
    Write-Host ''

    Push-Location $WorkingDirectory
    try {
        # Out-Host keeps the tool's own output off this function's pipeline, so
        # the caller receives only the boolean this function returns.
        & $Command @Arguments | Out-Host
        $code = $LASTEXITCODE
    } catch {
        Write-Host "    $($_.Exception.Message)" -ForegroundColor Red
        $code = 1
    } finally {
        Pop-Location
    }

    if ($code -eq 0) {
        Write-Host ''
        Write-Host "    PASS  $Name" -ForegroundColor Green
        Add-Result -Name $Name -Status 'PASS'
        return $true
    }

    Write-Host ''
    Write-Host "    FAIL  $Name  (exit code $code)" -ForegroundColor Red
    Add-Result -Name $Name -Status 'FAIL' -Note "exit code $code"
    $script:failed++
    return $false
}

# --------------------------------------------------------------------------

Write-Host ''
Write-Host '  QLabs REAPER Music Intelligence MCP - release validation' -ForegroundColor White
Write-Host '  ---------------------------------------------------------------'

if (-not $SourceRoot) { $SourceRoot = Split-Path -Parent $PSScriptRoot }
if (-not (Test-Path -LiteralPath (Join-Path $SourceRoot 'Cargo.toml') -PathType Leaf)) {
    throw "SourceRoot '$SourceRoot' does not contain Cargo.toml. Pass -SourceRoot <repo root>."
}
$SourceRoot = (Resolve-Path -LiteralPath $SourceRoot).ProviderPath
Write-Host "  repository : $SourceRoot"

$cargo = Get-Command 'cargo' -ErrorAction SilentlyContinue
if (-not $cargo) {
    throw 'cargo is not on PATH. Install a stable Rust toolchain and re-run.'
}

$keepGoing = $true

if ($keepGoing) {
    $ok = Invoke-Gate -Name 'Formatting' -WorkingDirectory $SourceRoot `
        -Command 'cargo' -Arguments @('fmt', '--all', '--check')
    if (-not $ok -and $StopOnFirstFailure) { $keepGoing = $false }
}

if ($keepGoing) {
    $ok = Invoke-Gate -Name 'Lints' -WorkingDirectory $SourceRoot `
        -Command 'cargo' -Arguments @('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings')
    if (-not $ok -and $StopOnFirstFailure) { $keepGoing = $false }
}

if ($keepGoing) {
    $ok = Invoke-Gate -Name 'Rust tests' -WorkingDirectory $SourceRoot `
        -Command 'cargo' -Arguments @('test', '--workspace')
    if (-not $ok -and $StopOnFirstFailure) { $keepGoing = $false }
}

if ($keepGoing) {
    $ok = Invoke-Gate -Name 'Knowledge, schemas and fixtures' -WorkingDirectory $SourceRoot `
        -Command 'cargo' -Arguments @('run', '-p', 'xtask', '--', 'check-all')
    if (-not $ok -and $StopOnFirstFailure) { $keepGoing = $false }
}

if ($keepGoing) {
    if ($SkipLua) {
        Write-Host ''
        Write-Host '==> REAPER bridge tests' -ForegroundColor Cyan
        Write-Host '    SKIP  -SkipLua was given' -ForegroundColor Yellow
        Add-Result -Name 'REAPER bridge tests' -Status 'SKIP' -Note '-SkipLua'
    } else {
        $lua = Get-Command 'lua5.4' -ErrorAction SilentlyContinue
        if (-not $lua) { $lua = Get-Command 'lua' -ErrorAction SilentlyContinue }
        if (-not $lua) {
            Write-Host ''
            Write-Host '==> REAPER bridge tests' -ForegroundColor Cyan
            Write-Host '    SKIP  lua5.4 is not installed' -ForegroundColor Yellow
            Write-Host '          This is a skip, not a pass. The 184 bridge cases'
            Write-Host '          were not run. Install Lua 5.4 and re-run to cover them.'
            Add-Result -Name 'REAPER bridge tests' -Status 'SKIP' -Note 'lua5.4 not installed'
        } else {
            $ok = Invoke-Gate -Name 'REAPER bridge tests' `
                -WorkingDirectory (Join-Path $SourceRoot 'reaper') `
                -Command $lua.Source -Arguments @('tests/run_tests.lua')
            if (-not $ok -and $StopOnFirstFailure) { $keepGoing = $false }
        }
    }
}

$exePath = $null

if ($keepGoing) {
    if ($SkipRelease) {
        Write-Host ''
        Write-Host '==> Release build' -ForegroundColor Cyan
        Write-Host '    SKIP  -SkipRelease was given' -ForegroundColor Yellow
        Add-Result -Name 'Release build' -Status 'SKIP' -Note '-SkipRelease'
    } else {
        $ok = Invoke-Gate -Name 'Release build' -WorkingDirectory $SourceRoot `
            -Command 'cargo' -Arguments @('build', '--workspace', '--release')
        if ($ok) {
            $candidate = Join-Path (Join-Path (Join-Path $SourceRoot 'target') 'release') $ExeName
            if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
                $candidate = Join-Path (Join-Path (Join-Path $SourceRoot 'target') 'release') $NixName
            }
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { $exePath = $candidate }
        } elseif ($StopOnFirstFailure) {
            $keepGoing = $false
        }
    }
}

if ($keepGoing -and $exePath) {
    $ok = Invoke-Gate -Name 'Binary starts' -WorkingDirectory $SourceRoot `
        -Command $exePath -Arguments @('version')
    if (-not $ok -and $StopOnFirstFailure) { $keepGoing = $false }
}

if ($keepGoing -and $exePath) {
    [void](Invoke-Gate -Name 'Embedded knowledge bundle' -WorkingDirectory $SourceRoot `
        -Command $exePath -Arguments @('validate-knowledge'))
}

# --------------------------------------------------------------------------
# Summary
# --------------------------------------------------------------------------

Write-Host ''
Write-Host '  ---------------------------------------------------------------'
Write-Host '  Summary' -ForegroundColor White
Write-Host '  ---------------------------------------------------------------'
foreach ($result in $results) {
    $colour = 'Green'
    if ($result.Status -eq 'FAIL') { $colour = 'Red' }
    if ($result.Status -eq 'SKIP') { $colour = 'Yellow' }
    $line = '   {0,-4}  {1}' -f $result.Status, $result.Name
    if ($result.Note) { $line = "$line  ($($result.Note))" }
    Write-Host $line -ForegroundColor $colour
}
Write-Host ''

if ($failed -gt 0) {
    Write-Host "  $failed gate(s) failed. This build is not releasable." -ForegroundColor Red
    Write-Host ''
    exit 1
}

$skipped = @($results | Where-Object { $_.Status -eq 'SKIP' }).Count
if ($skipped -gt 0) {
    Write-Host "  All gates that ran passed, but $skipped were skipped." -ForegroundColor Yellow
} else {
    Write-Host '  All gates passed.' -ForegroundColor Green
}
Write-Host ''
Write-Host '  Not covered by this script: the in-REAPER smoke test' -ForegroundColor Yellow
Write-Host '  (QLabs_Reaper_MCP_Smoke_Test.lua). It requires a real REAPER host'
Write-Host '  and must be run by hand. See docs\TESTING.md.'
Write-Host ''
exit 0
