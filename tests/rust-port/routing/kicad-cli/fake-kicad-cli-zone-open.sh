#!/bin/sh
# fake-kicad-cli.sh with native zone opens (zone-open).
MODE=zone-open exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
