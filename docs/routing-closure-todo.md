# Routing closure work list

Baseline: the matched four- and six-layer full-vertical experiments each
connected 45/58 nets. The six-layer draft left eight search-budget failures,
four `cannot reach` failures, and GND pending verified fill. It also contained
two same-net via drill-spacing violations; the added In3/In4 layers were unused.

- [x] Reserve dedicated inner GND-zone layers from foreign-net global,
      critical, fanout, and detailed routes.
- [ ] Stitch GND pads and verify the actual KiCad zone fill and continuity.
- [x] Reject or repair vias too close to other vias in the *same candidate
      net*, including branches created within one multi-terminal route.
- [x] Make global guides and detailed search use available signal layers when
      a direct front-layer path is congested, without adding forbidden
      transitions or destroying the return plane.
- [ ] Improve dense-package pad access with compatible escape choices and
      placement feedback; distinguish a genuinely blocked pad from a bounded
      search failure.
- [x] Model duplicated physical lands sharing one logical pad number (the
      TL3342 button has two separate `SW_USER.1` lands). Require a verified
      internal-tie declaration or a legal copper stitch so KiCad and native
      connectivity agree.
- [ ] Resolve the four still-pending duplicated connector shield lands and
      all GND contacts without relying on an unqualified internal tie.
- [x] Rerun matched four/six-layer experiments and KiCad DRC. Record open
      nets, layer usage, via count, and drill spacing.
- [ ] Resolve every open net, verify GND zone fill and all pad stitches, then
      run physical DRC and manufacturing/CAM signoff.

No generated board is manufacturing-ready until every connection and required
physical/electrical/CAM gate passes.

Progress: the layer reservation and tentative-via checks have focused
regression tests. With progressive guides, the detailed router now tries a
layer-projected geometric corridor before its unrestricted search; an
alternate-layer wall-crossing regression passes. A six-layer 1,000-state
smoke run connected 31/58 nets, matching the previous short-run scale, and
confirmed that non-GND global guides avoid In1.Cu. A matched six-layer
5,000-state run *before* the projected-guide fallback connected 43/58 nets;
the prior baseline was 45/58. Independent KiCad 10 DRC with zone refill
found no drill-spacing errors or copper shorts, but 153 unconnected items
and eight footprint-library findings.

The same settings with projected guides and GND-pad stitching produced:

| Draft | Routed nets | Signal-route vias | Pending GND pads | KiCad unconnected items |
|---|---:|---:|---:|---:|
| Four layers | 50/58 | 97 | 12 | 35 |
| Six layers | 51/58 | 96 | 8 | 30 |

Six-layer ordinary copper now uses F.Cu, In2.Cu, In3.Cu, In4.Cu, and B.Cu;
In1.Cu remains the GND plane. Both KiCad DRC reports show eight footprint
library findings, no copper shorts, and no drill-spacing violations.
With dense-pin fanout, bounded rip-up, and a 10x failed-net search repair,
the six-layer late-stitch draft routed all 57 signal nets. The new
duplicate-pad closure connected both button lands, and off-grid reuse of
existing GND vias cut new stitch drills from 66 to 50. KiCad 10 then
reported 14 GND unconnected items and zero signal gaps, copper shorts, or
drill-spacing violations. Eight GND pads and four duplicated connector
shield references remain pending in the closure stages; eight footprint
library lookup/mismatch warnings remain for qualification.

An experimental all-early plane stitch connected every targeted GND pad,
but blocked V3V3 (31 KiCad V3V3 gaps); selectively reserving the eight
previously pending GND pads also blocked V3V3. Early plane reservation is
therefore opt-in only. A broader three-candidate placement experiment with
two detailed-feedback trials again yielded 57/58 logical nets and 14 KiCad
GND opens. Those trials were targeting deliberately deferred GND; placement
feedback now excludes deferred plane nets and reserves a separate plane-aware
objective for future work. Ground escapes now try a bounded two-segment
Manhattan path when a direct pad-to-via trace is blocked, retaining exact
copper and board-edge checks. The first rerun improved the pending GND-pad
count from eight to seven, and KiCad's refilled-zone DRC improved from 14 to
13 GND opens without introducing copper shorts or drill-spacing violations.
Repeated connector shield lands now also try a bounded three-segment lateral
detour. The next rerun stitched J_CELL.2, J_GNSS.2, and J_SIM.SH; only
J_POWER.SH remained pending, and KiCad's open count fell from 13 to 9.
The plane stage now separately escapes every physical SMD land sharing a
logical pad number instead of silently taking the first; a full-board rerun
reduced KiCad's open count from 9 to 7 and resolved the USB-C shield gap.
The remaining seven opens are GND contacts around the CC controller, MCU
decoupling/level shifter, and GNSS. KiCad reports no copper-short or
drill-spacing violations, but eight footprint-library warnings remain.
None of these drafts is production-ready.

A 0.25 mm candidate grid and 5 mm plane-via search radius (versus the
default 0.5 mm/3 mm) reduced KiCad's open count from seven to five, but
introduced one new F.Cu copper-sliver warning. The larger/finer search is
therefore experimental rather than a clean improvement. A bounded
three-segment pad-to-via detour is being compared on the original coarse
grid. The seven-open default-grid draft remains the cleanest independent
DRC result aside from eight footprint-library warnings.

The 1.5 mm three-segment detour on the original coarse grid did not change
the seven-open KiCad count, so it remains opt-in. A 0.12 mm local GND escape
width also left seven opens. Its first draft exposed a project-rule bug:
KiCad inherited a 0.20 mm *minimum* width and rejected all 85 narrow traces.
Minimum track width is now first-class in the physical IR, native DRC, signed
geometry digest, fabrication profile, and exported KiCad project. The
regenerated 0.12 mm draft passed KiCad's corrected width rule, but still had
seven GND opens and eight footprint-library warnings.

A bounded same-net pad-to-escaped-pad rescue can build short surface chains
to one legal plane via instead of requiring a via at every GND pad. Its 5 mm
full-board experiment added one surface track but left seven KiCad opens and
eight footprint-library warnings. It is opt-in (default contact radius zero)
until plane-aware placement, a more capable legal access search, or a targeted
site move shows an actual improvement on this board.

An opt-in six-layer GND via-in-pad experiment is now available with
`--ground-via-in-pad` in the stitched route flow. It attempts a 0.30/0.20 mm
filled-and-capped via at the pad center only when ordinary escape fails, and
reports the manufacturing requirement separately from KiCad geometry. It must
pass independent zone-refilled KiCad DRC and a separately confirmed JLCPCB
plated-over-filled via order before it can count toward board closure.
The first full-board pass provisionally closed U_CC.3 (six opens), but KiCad
found four hole-to-copper clearance errors at that via plus generic minimum
via-rule errors. The corrected planner and native DRC now enforce the board's
0.25 mm hole-to-copper rule; the matched six-layer rerun placed no filled via.
Independent KiCad 10 DRC after zone refill again reports seven GND opens and
eight footprint-library warnings, with no new copper or drill violations.
Completing these pads needs a layout/routing change or a separately reviewed
local fabrication rule, not merely permission to use via-in-pad.
