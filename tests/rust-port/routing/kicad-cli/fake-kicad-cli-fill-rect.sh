#!/bin/sh
# fake-kicad-cli.sh with rect native fills (fill.awk).
FILL=rect exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
