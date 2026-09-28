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
The six-layer draft still has six open signal nets (`GNSS_TX`, `I2C_SCL`,
`MCU_NRF_RX`, `MCU_NRF_TX`, `MODEM_EN`, `V3V8`) plus GND pending verified
fill. An additional bounded fanout/rip-up/search-repair experiment is in
progress. None of these drafts is production-ready.
