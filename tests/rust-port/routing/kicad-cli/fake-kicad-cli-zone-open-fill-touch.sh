#!/bin/sh
# fake-kicad-cli.sh with native zone opens and touching-hole fills.
MODE=zone-open FILL=touch exec /bin/sh "$(dirname "$0")/fake-kicad-cli.sh" "$@"
