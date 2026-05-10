#!/usr/bin/env bash
# Refresh the bundled models.dev snapshot.
#
# Run this when models.dev publishes new models / pricing and you want the
# default Claurst install to ship with the latest catalog without forcing
# every user to wait for the background network refresh.
#
#   ./scripts/sync-models.sh
#
# After running, commit the updated assets/models-snapshot.json.

set -euo pipefail

URL="${CLAURST_MODELS_URL:-${MODELS_DEV_URL:-https://models.dev/api.json}}"
FORCE="${FORCE:-0}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$repo_root/crates/api/assets/models-snapshot.json"

echo "Fetching $URL ..."
curl --fail --silent --show-error --max-time 30 -o "$dest" "$URL"

if command -v jq >/dev/null 2>&1; then
    provider_count=$(jq 'length' "$dest")
    if [ "$provider_count" -lt 50 ] && [ "$FORCE" != "1" ]; then
        echo "Aborting: snapshot only contains $provider_count providers (expected 50+)." >&2
        echo "Set FORCE=1 to override." >&2
        exit 1
    fi
    echo "✓ Snapshot saved with $provider_count providers."
else
    echo "(jq not installed — skipping sanity check)"
fi

echo "Wrote $dest"
echo "Now run: cargo test -p claurst-api --lib model_registry"
