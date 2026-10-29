# Writes native filled polygons into every net zone of a saved board, as KiCad's
# --refill-zones would, for the stand-in kicad-cli. The fills are fixed walks
# on a 20x20 grid over the zone outline's bounding box (rounded to 1 um), in
# KiCad's representations: simple rings, holes fractured into the outer walk
# by reversed-edge slits, and several filled polygons per zone. SHAPE picks
# the topology; each one reaches a different polygon decoding case.
function walk(text,    count, items, i, pair, gx, gy, result) {
    count = split(text, items, " ")
    result = ""
    for (i = 1; i <= count; i++) {
        split(items[i], pair, ",")
        gx = sprintf("%.3f", x0 + (x1 - x0) * pair[1] / 20)
        gy = sprintf("%.3f", y0 + (y1 - y0) * pair[2] / 20)
        result = result sprintf("        (xy %s %s)\n", gx, gy)
    }
    return result
}
function polygon(text) {
    return sprintf("    (filled_polygon\n      (layer \"%s\")\n      (pts\n%s      )\n    )\n", layer, walk(text))
}
function fills(    result) {
    if (SHAPE == "rect")
        return polygon("4,4 16,4 16,16 4,16")
    if (SHAPE == "slit")
        return polygon("4,4 16,4 16,16 4,16 4,10 8,10 8,12 12,12 12,8 8,8 8,10 4,10")
    if (SHAPE == "shared")
        # Two holes meeting at one vertex, entered by one slit.
        return polygon("4,4 16,4 16,16 4,16 4,7 7,7 7,10 10,10 10,13 13,13 13,10 10,10 10,7 7,7 4,7")
    if (SHAPE == "touch")
        # A hole whose vertex lies on the outer ring's bottom edge.
        return polygon("4,4 16,4 16,16 4,16 4,10 8,10 8,12 12,12 12,8 10,4 8,8 8,10 4,10")
    if (SHAPE == "islands")
        # Two faces joined by a slit, meeting again at one corner.
        return polygon("2,2 10,2 10,10 2,10 2,6 12,6 12,10 18,10 18,18 10,18 10,10 12,10 12,6 2,6")
    if (SHAPE == "crossing")
        # Two slit holes whose rings cross: invalid, never healed.
        return polygon("2,2 18,2 18,18 2,18 2,6 5,6 5,10 10,10 10,5 5,5 5,6 2,6 2,8 7,8 7,13 13,13 13,7 7,7 7,8 2,8")
    if (SHAPE == "overlap")
        # Two filled polygons of one zone that overlap in a band.
        return polygon("0,0 13,0 13,20 0,20") polygon("7,0 20,0 20,20 7,20")
    if (SHAPE == "overlap-shared")
        return polygon("0,0 13,0 13,20 0,20") \
            polygon("7,0 20,0 20,20 7,20 7,7 10,7 10,10 13,10 13,13 16,13 16,10 13,10 13,7 10,7 7,7")
    return ""
}
/^  \(zone/ { inzone = 1; x0 = ""; layer = ""; netted = 0 }
inzone && /^    \(net [1-9][0-9]*\)$/ { netted = 1 }
inzone && /^    \(layer "/ { layer = $2; gsub(/[")]/, "", layer) }
inzone && /^    \(layers "/ { layer = $2; gsub(/[")]/, "", layer) }
inzone && /^ +\(xy / {
    x = $2 + 0; y = $3; sub(/\)$/, "", y); y = y + 0
    if (x0 == "") { x0 = x; x1 = x; y0 = y; y1 = y }
    if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y
}
# Only net zones fill: keepouts and rule areas have no copper.
inzone && /^  \)$/ { if (netted) printf "%s", fills(); inzone = 0 }
{ print }
