#!/bin/sh
# fake-kicad-cli.sh with one injected failure (drc-violations).
MODE=drc-violations exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
