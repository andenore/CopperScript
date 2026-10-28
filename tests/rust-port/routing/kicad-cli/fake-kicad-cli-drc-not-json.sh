#!/bin/sh
# fake-kicad-cli.sh with one injected failure (drc-not-json).
MODE=drc-not-json exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
