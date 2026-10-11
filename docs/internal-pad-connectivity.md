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
