#!/bin/sh
# The manufacturing fake kicad-cli as route-board --verify-plane-fill calls it:
# it answers --version and takes the DRC report path from --output. MODE (set
# by the sibling wrappers) injects one failure; FILL (set by the fill-*
# wrappers) writes native filled polygons of that shape into the saved board.
here=$(dirname "$0")
if [ "$1" = --version ]; then
    [ "${MODE:-ok}" = version-fails ] && { echo "kicad-cli: cannot start" >&2; exit 3; }
    [ "${MODE:-ok}" = version-nine ] && { echo "9.0.4"; exit 0; }
    echo "10.0.7"
    exit 0
fi
board=
count=$#
while [ "$count" -gt 0 ]; do
    argument=$1
    shift
    [ "$argument" = --output ] && argument=-o
    set -- "$@" "$argument"
    board=$argument
    count=$((count - 1))
done
[ -z "$FILL" ] && exec /bin/sh "$here/../../manufacturing/kicad-cli/fake-kicad-cli.sh" "$@"
/bin/sh "$here/../../manufacturing/kicad-cli/fake-kicad-cli.sh" "$@" || exit
awk -v SHAPE="$FILL" -f "$here/fill.awk" "$board" > "$board.filled" && mv "$board.filled" "$board" || exit 2
