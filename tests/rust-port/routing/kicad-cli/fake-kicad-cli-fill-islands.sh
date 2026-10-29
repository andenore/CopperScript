#!/bin/sh
# fake-kicad-cli.sh with islands native fills (fill.awk).
FILL=islands exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
