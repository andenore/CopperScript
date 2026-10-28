#!/bin/sh
# fake-kicad-cli.sh with a zone open, then a dangling end (zone-open-dangling).
MODE=zone-open-dangling exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
