#!/bin/sh
# fake-kicad-cli.sh with native zone opens (zone-open-always).
MODE=zone-open-always exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
