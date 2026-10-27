#!/bin/sh
# fake-kicad-cli.sh with one injected failure (missing-gerber).
MODE=missing-gerber exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
