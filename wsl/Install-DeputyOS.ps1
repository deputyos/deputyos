<#
.SYNOPSIS
    Imports the deputyOS WSL2 distro tarball into Windows Subsystem for
    Linux 2.

.DESCRIPTION
    Downloads the published deputyOS WSL2 tarball, verifies its
    checksum, and runs `wsl --import` to register it as the `deputyos`
    distro under $env:LOCALAPPDATA\deputyos. After import the user can:

        wsl -d deputyos
        deputyctl init

    Requires:
      - Windows 10 21H2+ or Windows 11
      - PowerShell 5.1+ (Windows PowerShell) or 7+ (PowerShell Core)
      - WSL2 already enabled (`wsl --install` if not)
      - ~3 GB disk space at $env:LOCALAPPDATA\deputyos

.PARAMETER Profile
    Profile id to install: openclaw (default) or hermes.

.PARAMETER Channel
    Release channel: stable (default), beta, or dev.

.PARAMETER Version
    Image version. Defaults to "latest" which the dist URL resolves to
    the current release.

.PARAMETER InstallDir
    Where WSL stores the distro VHDX. Default:
    $env:LOCALAPPDATA\deputyos.

.PARAMETER LocalTarball
    If supplied, skip the download and use this path instead. Used by
    `make build TARGET=wsl2` from a sibling Linux/WSL2 host.

.EXAMPLE
    PS> .\Install-DeputyOS.ps1
    Installs the latest stable OpenClaw WSL2 distro.

.EXAMPLE
    PS> .\Install-DeputyOS.ps1 -Profile hermes -Channel beta
    Installs the latest beta Hermes WSL2 distro.

.EXAMPLE
    PS> .\Install-DeputyOS.ps1 -LocalTarball C:\path\to\deputyos-openclaw-wsl2-dev.tar.gz
    Installs from a locally-built tarball (skips download).

.NOTES
    Project: deputyOS - see https://www.deputyos.com
    Source:  https://github.com/deputyos/deputyos
#>

[Diagnostics.CodeAnalysis.SuppressMessageAttribute(
    'PSAvoidUsingWriteHost',
    '',
    Justification = 'This is an interactive installer; progress belongs on the host UI.'
)]
[CmdletBinding()]
param(
    [ValidateSet('openclaw', 'hermes')]
    [Alias('Profile')]
    [string]$AgentProfile = 'openclaw',

    [ValidateSet('stable', 'beta', 'dev')]
    [string]$Channel = 'stable',

    [string]$Version = 'latest',

    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'deputyos'),

    [string]$LocalTarball = $null
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# ---- pre-flight ----
function Test-DeputyOSPrerequisite {
    param([string]$Destination)

    Write-Host '==> deputyOS WSL2 installer: pre-flight checks'

    if ($PSVersionTable.PSVersion.Major -lt 5) {
        throw 'PowerShell 5.1 or newer is required. Install Windows PowerShell or PowerShell 7 from https://aka.ms/powershell.'
    }

    $wsl = Get-Command 'wsl.exe' -ErrorAction SilentlyContinue
    if (-not $wsl) {
        throw 'wsl.exe not found. Run `wsl --install` from an elevated PowerShell, reboot, then try again.'
    }

    # Probe WSL2 default version. `wsl --status` works on 22H2+; older
    # builds need `wsl -l -v`.
    try {
        $statusRaw = & wsl.exe --status 2>&1 | Out-String
        if ($statusRaw -notmatch 'Default Version:\s*2') {
            Write-Warning 'WSL default version is not 2. Run `wsl --set-default-version 2` and retry.'
        }
    } catch {
        Write-Verbose 'wsl --status not available; skipping default-version probe.'
    }

    if (-not (Test-Path $Destination)) {
        New-Item -ItemType Directory -Path $Destination | Out-Null
    }

    # Refuse to clobber an existing import.
    $existing = & wsl.exe -l -q 2>&1
    if ($existing -match '^deputyos$') {
        throw 'A WSL distro named `deputyos` already exists. Run `wsl --unregister deputyos` to remove it before re-importing, or pass -InstallDir to choose a different name.'
    }
}

# ---- download / locate tarball ----
function Get-DeputyOSTarball {
    param(
        [string]$SelectedProfile,
        [string]$SelectedChannel,
        [string]$SelectedVersion,
        [string]$SourceTarball
    )

    if ($SourceTarball) {
        if (-not (Test-Path $SourceTarball)) {
            throw "LocalTarball not found: $SourceTarball"
        }
        Write-Host "==> using local tarball: $SourceTarball"
        return (Resolve-Path $SourceTarball).Path
    }

    $distRoot = 'https://cdn.deputyos.com'

    $name = "deputyos-$SelectedProfile-wsl2-$SelectedVersion-$SelectedChannel.tar.gz"
    $url = "$distRoot/$name"

    $cacheDir = Join-Path $env:TEMP 'deputyos-wsl2'
    if (-not (Test-Path $cacheDir)) {
        New-Item -ItemType Directory -Path $cacheDir | Out-Null
    }
    $dest = Join-Path $cacheDir $name

    Write-Host "==> downloading $url"
    Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing

    # Verify the matching .sha256 file. The dist publishes
    # <tarball>.sha256 alongside; we refuse to import without the
    # checksum.
    $shaUrl = "$url.sha256"
    $shaDest = "$dest.sha256"
    Invoke-WebRequest -Uri $shaUrl -OutFile $shaDest -UseBasicParsing

    $expected = (Get-Content $shaDest -First 1).Split(' ')[0].Trim().ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 -Path $dest).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        throw "SHA256 mismatch for ${dest}: expected ${expected}, got ${actual}. Refusing to import. Delete the tarball and retry."
    }
    Write-Host "==> SHA256 verified: $actual"

    return $dest
}

# ---- import ----
function Import-DeputyOS {
    param(
        [string]$TarballPath,
        [string]$Destination
    )

    Write-Host "==> wsl --import deputyos $Destination $TarballPath --version 2"
    & wsl.exe --import 'deputyos' $Destination $TarballPath --version 2
    if ($LASTEXITCODE -ne 0) {
        throw "wsl --import failed with exit code $LASTEXITCODE. Common causes: WSL not fully installed; insufficient disk space; tarball corrupt."
    }
}

# ---- main ----
try {
    Test-DeputyOSPrerequisite -Destination $InstallDir
    $tarball = Get-DeputyOSTarball `
        -SelectedProfile $AgentProfile `
        -SelectedChannel $Channel `
        -SelectedVersion $Version `
        -SourceTarball $LocalTarball
    Import-DeputyOS -TarballPath $tarball -Destination $InstallDir

    Write-Host ''
    Write-Host '==> deputyOS WSL2 imported successfully'
    Write-Host ''
    Write-Host 'Next steps:'
    Write-Host '  1. Launch the distro:'
    Write-Host '       wsl -d deputyos'
    Write-Host '  2. Inside the distro, finish setup:'
    Write-Host '       deputyctl init'
    Write-Host '  3. Open the wizard:'
    Write-Host '       http://localhost:8088   (from Windows host)'
    Write-Host ''
    Write-Host 'Limitations on WSL2 (see docs/14-limitations.md):'
    Write-Host '  - No audio: voice features are disabled.'
    Write-Host '  - No mDNS to LAN: deputyos.local works only on the Windows host.'
    Write-Host '  - Updates are re-imports, not A/B partition swaps.'
} catch {
    Write-Host ''
    Write-Error $_.Exception.Message
    exit 1
}
