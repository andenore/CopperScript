#!/usr/bin/env bash
# Copy a recorded Tracker showcase into site/assets, checking its asset
# bindings before writing anything.
#   scripts/update_site_showcase.sh --tracker-root ../CopperAssetTracker
set -euo pipefail
[ "${1:-}" = --tracker-root ] && [ -n "${2:-}" ] || { echo "usage: $0 --tracker-root DIR" >&2; exit 2; }
source="$(cd "$2/docs/showcase" && pwd)"
destination="$(cd "$(dirname "$0")/.." && pwd)/site/assets"
assets=(routing-top.svg routing-inner.svg routing-bottom.svg routing-all.svg placement.svg rendering.json
  electrical-ir.excerpt.json physical-ir.excerpt.json)
for name in "${assets[@]}"; do
  expected="$(jq -r --arg name "$name" '.rendered_assets[$name]' "$source/snapshot.json")"
  actual="$(sha256sum "$source/$name" | cut -d' ' -f1)"
  [ "$actual" = "$expected" ] || { echo "$name: hash mismatch; regenerate the Tracker showcase first" >&2; exit 1; }
done
mkdir -p "$destination"
for name in "${assets[@]}" snapshot.json; do cp "$source/$name" "$destination/$name"; done
echo "Copied recorded showcase. Review website captions, date and metrics before publishing."
