#!/bin/sh
# fake-kicad-cli.sh with shared native fills (fill.awk).
FILL=shared exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
