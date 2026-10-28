#!/bin/sh
# fake-kicad-cli.sh with one injected failure (version-fails).
MODE=version-fails exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
