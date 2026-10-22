#!/usr/bin/env bash
# ci_install_from_ppa.sh PPA SERIES PACKAGE...
#
# Install packages from a KiCad PPA, retrying while Launchpad is unreachable.
# Launchpad's PPA hosting is intermittently down: adding the PPA fails, or the
# index offers only Ubuntu's own (older) KiCad, or downloads fail. Each step
# is retried up to six times a minute apart, and the install goes ahead only
# once the candidate `kicad` is from SERIES (for example 9.0).
set -euo pipefail
ppa=$1 series=$2
shift 2
attempts=${KICAD_INSTALL_ATTEMPTS:-6}
cache=${KICAD_DEB_CACHE:-}
if [ -n "$cache" ] && ls "$cache"/kicad_"$series".*.deb >/dev/null 2>&1; then
  # Packages saved by an earlier run: no Launchpad needed.
  sudo apt-get -o Acquire::Retries=3 update || true
  extra=()
  for name in "$@"; do ls "$cache"/"${name}"_*.deb >/dev/null 2>&1 || extra+=("$name"); done
  if sudo apt-get install --yes --no-install-recommends "$cache"/*.deb "${extra[@]}"; then
    echo "::notice title=KiCad $series::installed from the CI package cache"
    exit 0
  fi
fi
save() {
  if [ -n "$cache" ]; then mkdir -p "$cache"; cp /var/cache/apt/archives/*.deb "$cache"/ 2>/dev/null || true; fi
}
for attempt in $(seq 1 "$attempts"); do
  if sudo add-apt-repository --yes --no-update "$ppa"; then break; fi
  if [ "$attempt" = "$attempts" ]; then
    echo "::error title=KiCad install failed::could not add $ppa after $attempts attempts"; exit 1
  fi
  sleep 60
done
for attempt in $(seq 1 "$attempts"); do
  sudo apt-get -o Acquire::Retries=3 update || true
  candidate=$(apt-cache policy kicad | awk '/Candidate:/ {print $2}')
  echo "::notice title=KiCad PPA $ppa (attempt $attempt)::candidate kicad $candidate"
  case "$candidate" in
    "$series".*)
      if sudo apt-get -o Acquire::Retries=3 -o APT::Keep-Downloaded-Packages=true install --yes --no-install-recommends "$@"; then
        save
        exit 0
      fi
      failure="install failed" ;;
    *) failure="candidate kicad is $candidate, not $series.x (Launchpad unreachable?)" ;;
  esac
  if [ "$attempt" = "$attempts" ]; then
    echo "::error title=KiCad install failed::after $attempts attempts: $failure"; exit 1
  fi
  echo "attempt $attempt: $failure; retrying in 60 s"
  sleep 60
done
