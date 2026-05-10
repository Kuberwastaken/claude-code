# Claurst installer for Windows (PowerShell).
#
# Usage (one-liner):
#   irm https://github.com/Kuberwastaken/claurst/releases/latest/download/install.ps1 | iex
#
# Or download and run locally:
#   Invoke-WebRequest https://github.com/Kuberwastaken/claurst/releases/latest/download/install.ps1 -OutFile install.ps1
#   .\install.ps1

[CmdletBinding()]
param(
    [string]$Version = "",
    [string]$Binary = "",
    [string]$InstallDir = "",
    [switch]$NoModifyPath,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'

$App = 'claurst'
$Repo = 'Kuberwastaken/claurst'

function Write-Info($msg)    { Write-Host $msg }
function Write-Success($msg) { Write-Host $msg -ForegroundColor Green }
function Write-Warn($msg)    { Write-Host $msg -ForegroundColor Yellow }
function Write-Err($msg)     { Write-Host $msg -ForegroundColor Red }
function Write-Muted($msg)   { Write-Host $msg -ForegroundColor DarkGray }

function Show-Usage {
@"
Claurst installer (Windows)

Usage: install.ps1 [options]

Options:
    -Help                   Show this help
    -Version <version>      Install a specific version (e.g., 0.0.9)
    -Binary <path>          Install from a local binary instead of downloading
    -InstallDir <path>      Override install location (default: %USERPROFILE%\.claurst\bin)
    -NoModifyPath           Don't add the install dir to user PATH

Examples:
    irm https://github.com/Kuberwastaken/claurst/releases/latest/download/install.ps1 | iex
    .\install.ps1 -Version 0.0.9
    .\install.ps1 -Binary C:\path\to\claurst.exe
"@
}

if ($Help) { Show-Usage; exit 0 }

# ----- Detect architecture -----
function Get-Arch {
    $procArch = $env:PROCESSOR_ARCHITECTURE
    if ($null -eq $procArch) { $procArch = '' }
    switch ($procArch.ToLower()) {
        'amd64'   { return 'x86_64' }
        'x86'     { return 'x86_64' }   # rare; ship 64-bit anyway
        'arm64'   {
            Write-Warn "ARM64 Windows is not currently supported in releases. Falling back to x86_64."
            return 'x86_64'
        }
        default   {
            Write-Warn "Unknown architecture '$procArch'. Defaulting to x86_64."
            return 'x86_64'
        }
    }
}

# ----- Resolve install directory -----
if ([string]::IsNullOrEmpty($InstallDir)) {
    $InstallDir = Join-Path $env:USERPROFILE ".claurst\bin"
}
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# ----- Resolve version (latest if not provided) -----
function Resolve-Version {
    if (-not [string]::IsNullOrEmpty($script:Version)) {
        return ($script:Version -replace '^v', '')
    }
    try {
        $resp = Invoke-RestMethod -UseBasicParsing -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ 'User-Agent' = 'claurst-installer' }
        $tag = $resp.tag_name
        if ([string]::IsNullOrEmpty($tag)) { throw "no tag_name in response" }
        return ($tag -replace '^v', '')
    } catch {
        Write-Err "Failed to fetch latest version from GitHub API: $_"
        exit 1
    }
}

# ----- Already-installed check -----
function Check-Existing($desiredVersion) {
    $existing = Get-Command claurst -ErrorAction SilentlyContinue
    if ($null -eq $existing) { return }
    try {
        $vline = (& claurst --version) 2>&1 | Select-Object -First 1
        $installed = ($vline -split '\s+')[-1]
    } catch {
        $installed = 'unknown'
    }
    if ($installed -eq $desiredVersion) {
        Write-Muted "Version $desiredVersion already installed at $($existing.Source)"
        Write-Muted "Use -Version to install a different one."
        exit 0
    }
    Write-Muted "Found existing claurst at $($existing.Source) (v$installed) - upgrading to v$desiredVersion"
}

# ----- Download & extract -----
function Download-And-Install($desiredVersion, $arch) {
    $archive = "claurst-windows-$arch.zip"
    $url = "https://github.com/$Repo/releases/download/v$desiredVersion/$archive"
    $tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("claurst-install-" + [System.Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmpRoot -Force | Out-Null

    $zipPath = Join-Path $tmpRoot $archive
    $extractDir = Join-Path $tmpRoot "extract"
    New-Item -ItemType Directory -Path $extractDir -Force | Out-Null

    Write-Info "Installing claurst v$desiredVersion (windows-$arch)"
    Write-Muted "Downloading $url"
    try {
        # Disable progress UI for a faster, less noisy download.
        $oldPref = $ProgressPreference
        $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zipPath
        $ProgressPreference = $oldPref
    } catch {
        Write-Err "Download failed: $_"
        Write-Info ("Check that release v$desiredVersion exists for windows-" + $arch + ":")
        Write-Info "  https://github.com/$Repo/releases/tag/v$desiredVersion"
        Remove-Item -Recurse -Force $tmpRoot -ErrorAction SilentlyContinue
        exit 1
    }

    Write-Muted "Extracting..."
    try {
        Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force
    } catch {
        Write-Err "Extract failed: $_"
        Remove-Item -Recurse -Force $tmpRoot -ErrorAction SilentlyContinue
        exit 1
    }

    $extractedExe = Join-Path $extractDir 'claurst.exe'
    if (-not (Test-Path $extractedExe)) {
        Write-Err "Archive did not contain expected binary 'claurst.exe'"
        Get-ChildItem -Recurse $extractDir | Format-Table FullName
        Remove-Item -Recurse -Force $tmpRoot -ErrorAction SilentlyContinue
        exit 1
    }

    Install-Binary $extractedExe
    Remove-Item -Recurse -Force $tmpRoot -ErrorAction SilentlyContinue
}

function Install-FromBinary {
    if (-not (Test-Path $script:Binary)) {
        Write-Err "Binary not found at $script:Binary"
        exit 1
    }
    Write-Info "Installing claurst from $script:Binary"
    Install-Binary $script:Binary
}

function Install-Binary($source) {
    $target = Join-Path $InstallDir 'claurst.exe'

    # The currently running claurst.exe (if any) holds an exclusive file lock on
    # Windows.  Try to swap by renaming the old one first.
    if (Test-Path $target) {
        $stale = "$target.old"
        if (Test-Path $stale) { Remove-Item -Force $stale -ErrorAction SilentlyContinue }
        try { Move-Item -Force $target $stale } catch { }
    }

    Copy-Item -Force $source $target
    Write-Success "Installed: $target"
}

# ----- PATH modification -----
function Add-ToUserPath {
    if ($NoModifyPath) { return }

    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($null -eq $current) { $current = '' }

    # Already on PATH?
    $paths = $current -split ';' | Where-Object { $_ -ne '' }
    foreach ($p in $paths) {
        if ($p.TrimEnd('\') -ieq $InstallDir.TrimEnd('\')) {
            Write-Muted "Install dir already on user PATH: $InstallDir"
            return
        }
    }

    if ([string]::IsNullOrEmpty($current)) {
        $newPath = $InstallDir
    } else {
        $newPath = $InstallDir + ';' + $current
    }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')

    # Make it visible in this session too so claurst --version works immediately.
    $env:Path = $InstallDir + ';' + $env:Path

    Write-Success ("Added " + $InstallDir + " to user PATH")
    Write-Muted "Open a new terminal for the change to take effect everywhere."
}

# ----- GitHub Actions hint -----
function GithubPathHint {
    if ($env:GITHUB_ACTIONS -eq 'true' -and -not [string]::IsNullOrEmpty($env:GITHUB_PATH)) {
        Add-Content -Path $env:GITHUB_PATH -Value $InstallDir
        Write-Info "Added $InstallDir to `$GITHUB_PATH"
    }
}

# ----- Main flow -----
if (-not [string]::IsNullOrEmpty($Binary)) {
    Install-FromBinary
} else {
    $arch = Get-Arch
    $desiredVersion = Resolve-Version
    Check-Existing $desiredVersion
    Download-And-Install $desiredVersion $arch
}

Add-ToUserPath
GithubPathHint

Write-Host ""
Write-Success "claurst is installed!"
Write-Host ""
Write-Muted  "Quickstart:"
Write-Muted  "  # Set an API key"
Write-Host   "  `$env:ANTHROPIC_API_KEY = 'sk-ant-...'"
Write-Host   ""
Write-Muted  "  # Open a new terminal, then:"
Write-Success "  claurst             "
Write-Muted  "  # or"
Write-Success "  claurst -p `"...`"      "
Write-Host   ""
Write-Muted  "Docs: https://github.com/$Repo"
