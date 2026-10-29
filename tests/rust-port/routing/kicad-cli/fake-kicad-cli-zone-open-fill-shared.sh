#!/bin/sh
# fake-kicad-cli.sh with native zone opens and shared-vertex fills.
MODE=zone-open FILL=shared exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
