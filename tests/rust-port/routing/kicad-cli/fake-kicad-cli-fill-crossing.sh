#!/bin/sh
# fake-kicad-cli.sh with crossing native fills (fill.awk).
FILL=crossing exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
