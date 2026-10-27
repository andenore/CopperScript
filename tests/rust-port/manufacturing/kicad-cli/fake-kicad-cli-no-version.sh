#!/bin/sh
# fake-kicad-cli.sh with one injected failure (no-version).
MODE=no-version exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
