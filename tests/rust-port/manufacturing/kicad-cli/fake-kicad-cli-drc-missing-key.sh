#!/bin/sh
# fake-kicad-cli.sh with one injected failure (drc-missing-key).
MODE=drc-missing-key exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
