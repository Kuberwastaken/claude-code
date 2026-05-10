# Refresh the bundled models.dev snapshot.
#
# Run this when models.dev publishes new models / pricing and you want the
# default Claurst install to ship with the latest catalog without forcing
# every user to wait for the background network refresh.
#
#   pwsh scripts/sync-models.ps1
#
# After running, commit the updated assets/models-snapshot.json.

param(
    [string]$Url = $env:CLAURST_MODELS_URL,
    [switch]$Force
)

if (-not $Url) {
    if ($env:MODELS_DEV_URL) {
        $Url = $env:MODELS_DEV_URL
    } else {
        $Url = "https://models.dev/api.json"
    }
}

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$dest = Join-Path $repoRoot "crates/api/assets/models-snapshot.json"

Write-Host "Fetching $Url ..."
try {
    Invoke-WebRequest -Uri $Url -OutFile $dest -UseBasicParsing -TimeoutSec 30
} catch {
    Write-Error "Failed to fetch ${Url}: $_"
    exit 1
}

# Sanity-check: file must be valid JSON with at least 50 providers.
try {
    $json = Get-Content $dest -Raw | ConvertFrom-Json
    $providerCount = ($json.PSObject.Properties | Measure-Object).Count
    if ($providerCount -lt 50) {
        if (-not $Force) {
            Write-Error "Aborting: snapshot only contains $providerCount providers (expected 50+). Pass -Force to override."
            exit 1
        }
    }
    Write-Host "✓ Snapshot saved with $providerCount providers."
} catch {
    Write-Error "Aborting: snapshot is not valid JSON: $_"
    exit 1
}

Write-Host "Wrote $dest"
Write-Host "Now run: cargo test -p claurst-api --lib model_registry"
