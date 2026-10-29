#!/bin/sh
# fake-kicad-cli.sh with overlap native fills (fill.awk).
FILL=overlap exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
