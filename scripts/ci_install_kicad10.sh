#!/usr/bin/env bash
# Install the pinned KiCad 10 Ubuntu packages from the KiCad 10.0 release PPA.
#
# Two things go wrong. Launchpad's PPA hosting is sometimes unreachable for
# a while (the index then offers only Ubuntu's own KiCad 7, or downloads
# fail), so each attempt is retried with a pause. And the PPA publishes only
# its newest build, so a pin goes stale when KiCad releases a point version;
# then the job fails with an annotation naming the versions on offer, so the
# pins in .github/board-toolchain.json can be bumped without the job log.
# KICAD_PINS names another pin file (the comparison installs the candidate's
# KiCad on both sides).
set -euo pipefail
pins=${KICAD_PINS:-.github/board-toolchain.json}
pin() { python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$pins" "$1"; }
expected=$(pin kicad_ubuntu_version)
package=$(pin kicad_ubuntu_package_version)
attempts=${KICAD_INSTALL_ATTEMPTS:-6}
versions() { apt-cache madison "$1" | awk '{print $3}' | sort -u | tr '\n' ' '; }
annotate() { sed 's/%/%25/g' | awk '{printf "%s%%0A", $0}'; }

# Adding the PPA asks Launchpad for its signing key, so it can fail too.
for attempt in $(seq 1 "$attempts"); do
  if sudo add-apt-repository --yes --no-update ppa:kicad/kicad-10.0-releases; then
    break
  elif [ "$attempt" = "$attempts" ]; then
    echo "::error title=KiCad install failed::could not add ppa:kicad/kicad-10.0-releases after $attempts attempts"
    exit 1
  fi
  sleep 60
done
log=$(mktemp)
for attempt in $(seq 1 "$attempts"); do
  sudo apt-get -o Acquire::Retries=3 update || true
  offered=$(versions kicad)
  echo "::notice title=KiCad 10 PPA (attempt $attempt)::kicad: ${offered:-none}; kicad-footprints: $(versions kicad-footprints)"
  if apt-cache madison kicad | awk '{print $3}' | grep -qxF "$package"; then
    if sudo apt-get -o Acquire::Retries=3 install --yes --no-install-recommends \
         "kicad=$package" "kicad-footprints=$package" make >"$log" 2>&1; then
      cat "$log"
      break
    fi
    cat "$log"
    failure="install failed: $(grep -E '^(E:|W:)' "$log" | tail -5 | tr '\n' ' ')"
  elif echo "$offered" | grep -q '~ubuntu'; then
    # The PPA answers but no longer publishes the pin: retrying cannot help.
    echo "::error title=KiCad pin not published::kicad=$package is not in the PPA; on offer: $offered. Update kicad_version, kicad_ubuntu_version and kicad_ubuntu_package_version in $pins."
    exit 1
  else
    failure="the PPA offered no KiCad 10 build (Launchpad unreachable?)"
  fi
  if [ "$attempt" = "$attempts" ]; then
    echo "::error title=KiCad install failed::after $attempts attempts: $(echo "$failure" | annotate)"
    exit 1
  fi
  echo "attempt $attempt: $failure; retrying in 60 s"
  sleep 60
done
actual=$(kicad-cli version)
if [ "$actual" != "$expected" ]; then
  echo "::error title=KiCad version::Expected KiCad $expected, installed $actual (Ubuntu package $package)"
  exit 1
fi
echo 'KICAD_CLI=/usr/bin/kicad-cli' >> "$GITHUB_ENV"
echo 'KICAD10_FOOTPRINT_DIR=/usr/share/kicad/footprints' >> "$GITHUB_ENV"
