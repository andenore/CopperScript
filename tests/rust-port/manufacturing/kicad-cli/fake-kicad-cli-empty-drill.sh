#!/bin/sh
# fake-kicad-cli.sh with one injected failure (empty-drill).
MODE=empty-drill exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
