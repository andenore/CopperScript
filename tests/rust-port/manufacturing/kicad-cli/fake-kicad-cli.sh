#!/bin/sh
# A deterministic stand-in for KiCad 10's kicad-cli, for comparing the
# export-manufacturing pipeline of both implementations without KiCad. It
# writes plausible files where kicad-cli would; it checks nothing. MODE
# (set by the sibling wrappers) injects one failure.
MODE=${MODE:-ok}
if [ "$1" = version ]; then
    [ "$MODE" = no-version ] && exit 0
    [ "$MODE" = version-fails ] && { echo "kicad-cli: cannot start" >&2; exit 3; }
    echo "10.0.7"
    exit 0
fi
output=
board=
layers=
previous=
for argument in "$@"; do
    case $previous in
        -o) output=$argument ;;
        --layers) layers=$argument ;;
    esac
    previous=$argument
    board=$argument
done
base=$(basename "$board")
base=${base%.*}
case "$1 $2 $3" in
"pcb drc "*)
    [ "$MODE" = drc-fails ] && { echo "Found 3 DRC violations" ; exit 5; }
    [ "$MODE" = drc-silent ] && exit 5
    if [ "$MODE" = drc-violations ]; then
        printf '{"violations": [{"type": "clearance"}], "unconnected_items": []}\n' > "$output"
    elif [ "$MODE" = drc-not-json ]; then
        printf 'DRC report\n' > "$output"
    elif [ "$MODE" = drc-missing-key ]; then
        printf '{"violations": []}\n' > "$output"
    else
        printf '{"$schema": "https://schemas.kicad.org/drc.v1.json", "violations": [], "unconnected_items": [], "schematic_parity": []}\n' > "$output"
    fi
    # --save-board after refilling zones: the native board changes.
    printf '\n' >> "$board"
    ;;
"pcb export gerbers")
    [ "$MODE" = gerbers-fail ] && { echo "Failed to plot Edge.Cuts" >&2; exit 2; }
    count=0
    for layer in $(echo "$layers" | tr ',' ' '); do
        count=$((count + 1))
        [ "$MODE" = missing-gerber ] && [ $count -eq 2 ] && continue
        name=$(echo "$layer" | tr '.' '_')
        printf 'G04 fake %s artwork for %s*\n%%FSLAX46Y46*%%\n%%MOMM*%%\nD10*\nX0Y0D02*\nX%d000000Y0D01*\nM02*\n' \
            "$layer" "$base" "$count" > "$output/$base-$name.gbr"
    done
    ;;
"pcb export drill")
    [ "$MODE" = no-drill ] && exit 0
    if [ "$MODE" = empty-drill ]; then
        : > "$output/$base-PTH.drl"
    else
        printf 'M48\n; fake plated holes\nMETRIC\nT1C0.300\n%%\nT1\nX10.0Y-20.0\nM30\n' > "$output/$base-PTH.drl"
    fi
    printf 'M48\n; fake non-plated holes\nMETRIC\n%%\nM30\n' > "$output/$base-NPTH.drl"
    printf 'Drill report for %s\n' "$base" > "$output/$base-drl.rpt"
    ;;
"pcb export ipcd356")
    [ "$MODE" = no-netlist ] && exit 0
    printf 'C  IPC-D-356 fake netlist for %s\nP  JOB\n999\n' "$base" > "$output"
    ;;
"pcb export pos")
    [ "$MODE" = pos-fails ] && { echo "pos export failed" >&2; exit 1; }
    awk -v mode="$MODE" '
        BEGIN { print "Ref,Val,Package,PosX,PosY,Rot,Side" }
        function flush() {
            if (ref != "" && !excluded)
                printf "\"%s\",\"%s\",\"%s\",%.4f,%.4f,%.4f,%s\n", ref, value, package, x, -y, rot, side
            ref = ""; value = ""; excluded = 0; placed = 0; side = "top"
        }
        /^  \(footprint / {
            flush()
            package = $2; gsub(/"/, "", package); sub(/.*:/, "", package)
            infoot = 1
            next
        }
        /^  \(/ { flush(); infoot = 0 }
        infoot && /^    \(layer "B\.Cu"\)/ { side = "bottom" }
        infoot && /^    \(at / && !placed { x = $2; y = $3; rot = ($4 == "" ? 0 : $4); sub(/\)$/, "", y); sub(/\)$/, "", rot); placed = 1 }
        infoot && /^    \(property "Reference" / { ref = $3; gsub(/"/, "", ref) }
        infoot && /^    \(property "Value" / { value = $0; sub(/^[^"]*"Value" "/, "", value); sub(/".*$/, "", value) }
        infoot && /exclude_from_pos_files/ { excluded = 1 }
        END { flush(); if (mode == "pos-duplicate") print "\"DUP\",\"x\",\"y\",1.0000,2.0000,0.0000,top\n\"DUP\",\"x\",\"y\",1.0000,2.0000,0.0000,top" }
    ' "$board" > "$output"
    ;;
*)
    echo "fake kicad-cli: unexpected command: $*" >&2
    exit 2
    ;;
esac
exit 0
