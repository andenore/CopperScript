# Fixed copper polygons in physical hard macros

The TPS62130A probe needs precisely shaped, netted local copper that does not
change during zone refill. A macro-owned `CopperZone` remains appropriate for
plane/return pours; a fixed polygon is a separate, single-layer conductor.

Implemented in the v0.3 asset and the single-regulator probe. Native KiCad
refill/DRC and both regulator variants at four rotations are regression checks.

1. Add a v0.3 asset `polygons` list with identity, net, layer, and a simple
   closed vertex ring. Bind it to the asset digest and rigidly transform it
   with the macro. Reject self-intersections, zero-area contours, unsupported
   layers, and shapes outside the protected region.
2. Materialize polygons as immutable board copper. Source recovery must remove
   exactly the owned instances. Include them in geometry fingerprints and the
   explicit copper-connectivity proof, including pad, track, and via contacts.
   No zone outline may count as fixed copper.
3. Export locked, netted, filled KiCad 10 `gr_poly` objects. Check spacing and
   board-edge clearance against fixed polygons in CopperScript, then require
   native KiCad DRC on the minimal probe. Reject host routing inside macro
   reservations; never use a polygon as a substitute for a broad board plane.
4. Replace one awkward short conductor in the TPS62130A asset with a fixed
   polygon, regenerate both variants, and check all allowed rotations. Keep
   the switching-node area small. Verify the output board contains the netted
   polygon before and after refill, with no native DRC violations or opens.

No arbitrary polygon router, arcs, holes, mirrored instances, or vendor CAD
import are in scope. Thermal/current/EMI qualification remains separate.
