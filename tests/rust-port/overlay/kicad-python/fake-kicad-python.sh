#!/bin/sh
# Stands in for KiCad's Python: runs the probe stand-in on the board (the last argument).
exec python3 -I "$(dirname "$0")/native_probe_stub.py" "$@"
