#!/bin/sh
# fake-kicad-cli.sh with one injected failure (drc-silent).
MODE=drc-silent exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
