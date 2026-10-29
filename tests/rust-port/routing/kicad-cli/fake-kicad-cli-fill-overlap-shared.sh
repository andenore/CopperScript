#!/bin/sh
# fake-kicad-cli.sh with overlap-shared native fills (fill.awk).
FILL=overlap-shared exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
