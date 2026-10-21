# Permanent component-internal pad connectivity

Implementation and verification, 2026-10-03. See accepted decision CS-141 and
the language reference's `internal_pad_groups` property.

The Keystone 3034 positive solder tabs are parts of one conductive retainer;
its negative PCB battery contact is separate. E-Switch's TL3342 drawing shows
two permanent terminal pairs, joined to each other only when pressed. KiCad's
TL3342 footprint maps these to repeated pad numbers 1 and 2. CopperLib declares
`"1"` for the battery and `"1; 2"` for the switch, not one group across both
switch contacts.

Sources: [Keystone catalogue page 9](https://www.keyelco.com/userAssets/file/M65p9.pdf),
[E-Switch P010632 rev J drawing](https://configured-product-images.s3.amazonaws.com/2D/specs/TL3342F160QG.pdf),
[KiCad 10 jumper-pad documentation](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html#_jumper_pads).
CopperLib records drawing hashes and an agent review checklist. This is not a
general internal-circuit simulator or a source-reference requirement.

## Required external contacts

Permanent internal joins need manufacturer evidence; repeated pad numbers alone
are insufficient. Retain every solder/paste land and all externally required
power/thermal contacts. Prefer legal surface or off-pad ground access; a large
contact alone does not justify a via array or waive contact finish/process review.
Library-owned evidence and review rules are in CopperLib's internal connectivity
guide. Board-specific current validation belongs in [the nRF52 example](nrf52-coin-cell.md).

## Routing repeated lands without a declaration

Repeated pad numbers without an `internal_pad_groups` declaration are one
electrical pin, but KiCad (like Altium and, by default, Eagle) needs copper
between the lands: its ratsnest and DRC report "missing connection" until
they are joined. So the detailed router treats every separate land as its
own terminal of the net and routes to it, with vias when needed. Lands that
already touch or overlap (a split thermal pad, a slotted outline) are one
piece of copper and get no route. Declared groups are never routed between
their lands: one external contact satisfies them, and KiCad 10 receives
them as `jumper_pad_groups`.

The land stitcher still closes short surface gaps that remain. A land left
open is reported on its net: "lands of J1.1 are not joined by copper; declare
internal_pad_groups if the part joins them internally". When a part's
drawing shows the lands joined inside it (a tactile switch's terminal pairs,
a battery retainer), declare the group rather than routing every land.
