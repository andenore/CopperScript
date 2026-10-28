#!/bin/sh
# The manufacturing fake kicad-cli as route-board --verify-plane-fill calls it:
# it answers --version and takes the DRC report path from --output. MODE (set
# by the sibling wrappers) injects one failure.
here=$(dirname "$0")
if [ "$1" = --version ]; then
    [ "${MODE:-ok}" = version-fails ] && { echo "kicad-cli: cannot start" >&2; exit 3; }
    [ "${MODE:-ok}" = version-nine ] && { echo "9.0.4"; exit 0; }
    echo "10.0.7"
    exit 0
fi
count=$#
while [ "$count" -gt 0 ]; do
    argument=$1
    shift
    [ "$argument" = --output ] && argument=-o
    set -- "$@" "$argument"
    count=$((count - 1))
done
exec /bin/sh "$here/../../manufacturing/kicad-cli/fake-kicad-cli.sh" "$@"
