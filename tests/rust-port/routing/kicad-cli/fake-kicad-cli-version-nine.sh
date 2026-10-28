#!/bin/sh
# fake-kicad-cli.sh with one injected failure (version-nine).
MODE=version-nine exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
