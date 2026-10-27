#!/bin/sh
# fake-kicad-cli.sh with one injected failure (pos-duplicate).
MODE=pos-duplicate exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
