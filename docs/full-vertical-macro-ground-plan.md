# Full-vertical macro and top-ground integration

The existing TPS62130A modem buck is the board's only defined switching stage;
`PWR/U_LOGIC` is a generic three-terminal 3.3 V regulator placeholder.
Integrate the reviewed CopperLib enabled buck asset without replacing or
redrawing its protected copper. The
Nordic/Johanson physical asset contains a five-element RF path, while the
previous board had only two elements, so the electrical path must match the
asset before binding. Both assets remain prototype geometry, not qualified
power or RF designs.

1. Match the buck asset's nine members: use the 0402 AVIN capacitor, add its
   second 22 uF output capacitor, and bind all seven net roles. Keep the
   existing 3.8 V divider and enable pull-down. Add its required filled/capped
   exposed-pad via permission and an In4.Cu GND plane for its return contract.
2. Add the three Johanson tee components/nets to the Nordic RF path, bind the
   seven-member macro, and move its anchor to the board's upper-right corner.
   Stop applying the old three-member placement template and antenna pose, which
   conflict with the full hard macro.
3. Add F.Cu GND fill to the rest of the board. Leave the fixed buck's private
   F.Cu region unpoured; preserve the RF macro's own fill exclusions around
   chip matching, feed and antenna. Use several same-net board zones because
   the current source-zone language has no holes or macro-relative cutouts.
   Give intersecting same-net zones distinct KiCad priorities.
4. Check source/ERC, asset identities, macro materialization, legal placement,
   native KiCad fill/DRC and routing. Compare with the preceding routed board.
   Keep thermal/current and 2.4 GHz tuning claims out of geometric signoff.

The three-candidate placement search is used for this example, with
`candidate-00` selected for the default route. Explicit
30 mm placement bounds from FL_USB to both USB hosts keep the four-line filter
between the chips. A broad 20 x 20 mm MCU region preserves the upper USB lane
while allowing the optimizer to move the MCU within it. The first complete
macro/ground run passed native DRC but increased four-track USB length from
54.82 to 87.44 mm. The revised automatic choice (`candidate-02`) reduced
this to 83.25 mm. The verified `candidate-00` route reduced it further to
60.80 mm, with all 50 ordinary nets routed and zero filled-board KiCad
violations or unconnected items. Its board is in
`build/full-vertical/runs/20261009T092725319855Z/board.kicad_pcb`.

The buck asset's copper budget was developed for outputs below 0.5 A; this
board's modem has a 2 A peak requirement. A passing route cannot qualify its
VIN/VOUT copper, via current, thermal performance or transient response at that
load. The Nordic asset likewise needs matching and antenna testing on this
exact outline and stackup before hardware release.

The full-vertical scenes bind local copies of the CopperLib copper assets to
the board's pinned KiCad footprint revision. Copper pad locations and shapes
were checked against the installed-footprint source used by CopperLib; QFN
courtyards/body bounds and exposed-pad thermal metadata differ. The adapted
files record the source asset SHA-256 and are themselves digest-bound. They
change member footprint identities only, not the original assets' copper geometry.
