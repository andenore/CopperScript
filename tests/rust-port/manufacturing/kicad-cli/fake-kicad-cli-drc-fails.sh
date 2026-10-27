#!/bin/sh
# fake-kicad-cli.sh with one injected failure (drc-fails).
MODE=drc-fails exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
