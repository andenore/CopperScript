#!/bin/sh
# fake-kicad-cli.sh with touch native fills (fill.awk).
FILL=touch exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
