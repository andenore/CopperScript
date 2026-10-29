#!/bin/sh
# fake-kicad-cli.sh with slit native fills (fill.awk).
FILL=slit exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
