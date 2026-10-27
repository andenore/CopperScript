#!/bin/sh
# fake-kicad-cli.sh with one injected failure (no-drill).
MODE=no-drill exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
