#!/bin/sh
# fake-kicad-cli.sh with one injected failure (pos-fails).
MODE=pos-fails exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
