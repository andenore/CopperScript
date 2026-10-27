#!/bin/sh
# fake-kicad-cli.sh with one injected failure (no-netlist).
MODE=no-netlist exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
