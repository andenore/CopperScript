#!/bin/sh
# fake-kicad-cli.sh with a native dangling end (dangling).
MODE=dangling exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
