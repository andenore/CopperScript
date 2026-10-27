#!/bin/sh
# fake-kicad-cli.sh with one injected failure (gerbers-fail).
MODE=gerbers-fail exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
