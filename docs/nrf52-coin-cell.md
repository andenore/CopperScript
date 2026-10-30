# nRF52 coin-cell hard-macro example

`examples/nrf52_coin_cell/board.copper` is a small powered electrical example:
nRF52832 QFAA, the existing Nordic/Johanson RF hard macro, a 10-pin Samtec
Cortex-M SWD connector, two LEDs, two active-low buttons and a Keystone 3034
CR2032 holder. The orderable holder and reusable nRF parts live in CopperLib's
canonical vendor packages; illustrative crystal/LED support remains under
`examples/packages/nrf52_support`,
not in the compiler repository.

## Reproduce

Permanent battery/switch contact groups and their routing verification are
documented in [internal pad connectivity](internal-pad-connectivity.md). They
remove redundant airwires without inventing PCB copper; bare-board continuity
and installed-component continuity remain distinct.

CopperLib parts, footprints and macro assets download automatically from their
pinned GitHub revisions through `copper.mod`/`copper.lock`. No library checkout or
lock refresh is required. Run from CopperScript's root:

```sh
copper check examples/nrf52_coin_cell/board.copper --locked
make EXAMPLE=nrf52 layout
```

`build/nrf52-coin-cell/` is ignored by Git. `layout` writes the placed draft
(`placed.kicad_pcb`) and its placement report there. The former Python example
builder was removed with the Python implementation; the Make targets use the
same source declarations. Add `--offline` to `check` after the cache is
populated. Never refresh a lock just to suppress an unexplained mismatch.

## What is implemented

The provisional board is 50 x 40 mm. The coin-cell holder is on the left,
SWD at the upper left, antenna at the upper-right corner, and user controls
below the radio. All non-macro poses are explicit source placement constraints;
the seven-member macro is transformed from its fixed U_NRF anchor. Physical
lowering checks placement legality before transactional copper materialization.

The existing asset requires six layers, so this example keeps its exact layer
contract rather than claiming a qualified, cheaper two-layer adaptation. It
locks 20 RF/return tracks, two off-pad vias and seven member footprints. Macro
SHA, footprint identities, C3's protected VSS31 return, antenna fill exclusions
and isolated antenna pad 2 are retained unchanged. Electrical connectivity
remains exclusively in the `.copper` file; physical assets cannot redefine it.

The board imports the macro asset URL and declares all member/net bindings in
`physical_macro nordic-antenna-trial`. Dependency hashes come from the existing
package lock. The Make targets use that source declaration without separate
scene files or `--hard-macro` arguments. The full design is consumed, so source
artwork is retained; branding is
placed on the back near the lower edge to clear the SWD header's silk and pads.

`layout` is a placed draft. To attempt the full standard routing pipeline with
native refill and DRC:

```sh
make EXAMPLE=nrf52 route KICAD_CLI="C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
```

The run preserves immutable RF copper, uses the actual off-pad ground via
as a macro boundary terminal and retains an In1.Cu ground-fill intent. Reports are
saved on partial completion; exit 1 means failed gates.

## Latest documented status

The checkpoint under `build/nrf52-internal-contacts/` passes all 15 ordinary
package exits and independent KiCad 10.0.6 refill/DRC with zero unconnected items,
isolated islands or other violations. It preserves RF ownership and one explicit
BT1.NEG 0.30/0.20 mm filled-capped contact. Specify that process in fabrication;
it does not permit via-in-pad elsewhere.

The recorded CLI result remains failed/incomplete because its physical IR does
not import filled-plane polygons and retains deferred GND/open-route findings.
The native result is external connectivity evidence, not a fabricated IR signoff.
No qualified manufacturing release is claimed. New inputs require a new run.

See [hard-macro contract](physical-hard-macros.md) and [internal connectivity](internal-pad-connectivity.md).

## Circuit and firmware assumptions

The installed primary CR2032 is represented as a nominal 3 V external supply.
The passive holder alone is not a power source. There is no charger, external
power input or regulator. SWD pin 1 is **target-voltage sense**, not a charging
or power-injection input: configure the debug probe accordingly. Disconnect
the primary cell before attaching any external powered supply.

The LDO support circuit follows [Nordic's QFN48 LDO reference circuitry and
Table 1](https://docs.nordicsemi.com/r/bundle/ps_nrf52832/page/ref_circuitry.html):
DEC1 100 nF, DEC3 100 pF, DEC4 1 uF, VDD bypasses 100 nF and bulk 4.7 uF,
32 MHz crystal with nominal 12 pF load capacitors. DEC2/C6 is unpopulated and
DCC is unused. Firmware must leave DC/DC disabled, configure P0.21 as reset
through both matching [UICR PSELRESET registers](https://docs.nordicsemi.com/r/bundle/ps_nrf52832/page/uicr.html),
and use/calibrate the [internal low-frequency RC oscillator](https://docs.nordicsemi.com/r/bundle/ps_nrf52832/page/clock.html)
since the optional 32.768 kHz crystal is omitted. Apply the errata applicable
to the actual silicon revision; this example contains no firmware.

LED1=P0.13 and LED2=P0.14 are active-high through 4.7 kohm resistors. Choose
low-current LEDs suited to the falling cell voltage; ERC does not check LED
brightness/current. Button1=P0.11 and Button2=P0.12 have external 100 kohm
pull-ups and close to GND; configure them as inputs and debounce in firmware.
No peripheral assignment or firmware generation is implied.

The [Keystone 3034](https://www.keyelco.com/product.cfm/product_id/798) accepts
a CR2032. Battery pulse-current capability, voltage sag during radio bursts and
usable discharge range require checking against the chosen cell's datasheet
and hardware measurements. Nominal-voltage ERC is not that verification.

The crystal and LEDs are **explicit generic reference parts**, not verified
orderable devices. Choose a crystal meeting Nordic's CL=8 pF and frequency
tolerance requirements, verify its 1/3 signal and 2/4 ground pinout, and tune
load capacitors for board parasitics. Passive tolerances/materials, exact RF
component choices, the six-layer impedance/return structure and Johanson
antenna matching remain unqualified. This circuit does not copy the complete
vendor power/crystal layout into the RF macro and needs that layout review too.
