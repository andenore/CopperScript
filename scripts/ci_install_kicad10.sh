#!/usr/bin/env bash
# Install the pinned KiCad 10 Ubuntu packages from the KiCad 10.0 release PPA.
#
# The PPA publishes only its newest build, so a pin goes stale when KiCad
# releases a point version or the PPA rebuilds one. When that happens, fail
# with an annotation naming the versions on offer, so the pins in
# .github/board-toolchain.json can be bumped without reading the job log.
# KICAD_PINS names another pin file (the comparison installs the candidate's
# KiCad on both sides).
set -euo pipefail
pins=${KICAD_PINS:-.github/board-toolchain.json}
pin() { python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$pins" "$1"; }
expected=$(pin kicad_ubuntu_version)
package=$(pin kicad_ubuntu_package_version)

sudo add-apt-repository --yes ppa:kicad/kicad-10.0-releases
sudo apt-get update
versions() { apt-cache madison "$1" | awk '{print $3}' | sort -u | tr '\n' ' '; }
offered=$(versions kicad)
echo "::notice title=KiCad 10 PPA::kicad: ${offered:-none}; kicad-footprints: $(versions kicad-footprints)"
if ! apt-cache madison kicad | awk '{print $3}' | grep -qxF "$package"; then
  echo "::error title=KiCad pin not published::kicad=$package is not in the PPA; on offer: ${offered:-none}. Update kicad_version, kicad_ubuntu_version and kicad_ubuntu_package_version in $pins."
  exit 1
fi
log=$(mktemp)
if ! sudo apt-get install --yes --no-install-recommends "kicad=$package" "kicad-footprints=$package" make >"$log" 2>&1; then
  cat "$log"
  # The apt error, as one annotation (newlines encoded for the workflow command).
  detail=$(grep -E '^(E:|W:| )|Depends:|but it is not|unmet|held' "$log" | tail -25 | sed 's/%/%25/g' | awk '{printf "%s%%0A", $0}')
  echo "::error title=KiCad install failed::$detail"
  exit 1
fi
cat "$log"
actual=$(kicad-cli version)
if [ "$actual" != "$expected" ]; then
  echo "::error title=KiCad version::Expected KiCad $expected, installed $actual (Ubuntu package $package)"
  exit 1
fi
echo 'KICAD_CLI=/usr/bin/kicad-cli' >> "$GITHUB_ENV"
echo 'KICAD10_FOOTPRINT_DIR=/usr/share/kicad/footprints' >> "$GITHUB_ENV"
