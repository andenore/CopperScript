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

## Ground contact findings

These inspected published layouts demonstrate alternatives, not qualified
manufacturing recipes:

| Design | Battery negative contact | Observed GND connection |
| --- | --- | --- |
| [MVS adapter](https://github.com/bodgit/mvs-battery-holder) | 3.96 mm square | Surface trace to plated header; no contact vias |
| [Redox wireless rev1](https://github.com/mattdibi/redox-keyboard/tree/master/redox-w/rev1.0W/pcb) | 3.96 mm square | Surface GND pours; no contact vias |
| [Morse Code Watch](https://github.com/bnezuld/MorseCodeWatchPCB) | 17.8 mm disc | Four GND via centres inside contact, one more annulus overlaps its rim; surface GND copper too |

There is no demonstrated universal via count. Prefer a legal surface connection
or a nearby outside-contact via. Our prototype retains its explicit BT1.NEG
filled/capped via-in-pad permission (one 0.30/0.20 mm via). Do not create a via
array solely because a pad is large. Contact finish/mechanics, battery burst
current, voltage drop, return impedance and manufacturing process need review.

## Verification

Regressions cover DSL/bundle validation, alias voltage checks, assembly versus
bare copper, disabled unassembled paths, alternative-land escape/detailed
access, group-aware plane stitching, conflicting nets, native KiCad PCB DRC
and native schematic netlist preservation. Undeclared duplicate lands still
require copper closure. All solder/paste geometry remains present.

A profiled rerun with the updated local CopperLib declarations produced
`build/nrf52-internal-contacts/route-report.json` in about 397 s:

- All 15 ordinary package escapes passed, with no pending plane contacts.
- No redundant duplicate-pad bridge tracks were added.
- Independent KiCad 10.0.6 refill/DRC: **zero unconnected items, zero islands,
  zero other violations** (previous run: two airwires).
- One filled/capped BT1 ground via remains an explicit fabrication requirement.
- Signal tracks are straight or 45-degree, with zero other-angle length.

The overall route-board command still reports `fail`: native IR has no imported
filled-plane polygons and deliberately retains `DRC-OPEN-NET` for deferred GND
and `DRC-ROUTE-INCOMPLETE`. External plane verification is a separate evidence
record, not a fabricated native fill/signoff token. This run proves the contact
fix and clean exported-board electrical connectivity, **not production readiness**.
RF, mechanical, battery and manufacturing qualifications remain outstanding.

The verification fixture's local library replacement is ignored build data.
Published examples retain URL imports. CopperLib revision
`b336e61bd881287e7bed26c3929f213ec413f52b` is published and pinned in
`copper.mod`/`copper.lock`, so a clean checkout downloads these declarations
without requiring a manual CopperLib checkout.
