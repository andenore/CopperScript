# Round MCU / CR2032 LED-ring example

`examples/round_led_ring.copper` defines a small inspection design: twelve
independent active-low LED channels, STM32G0C1RET6, Keystone 3034 CR2032 holder,
10-pin Cortex-M SWD header, bypass/bulk capacitors and reset network. All device
parts reuse the pinned GitHub CopperLib dependency; no sibling checkout or new
compiler-local part definitions are required.

The physical builder produces a **50 mm true circular board**. LEDs are on the
front at radius 22 mm, clockwise from 12 o'clock, at 30-degree intervals. Their
10 kohm resistors form an inner ring at radius 18.2 mm. The MCU is centred on
the front and the battery holder centred on the back. SWD and bypass/reset
components have explicit source placement constraints. Ring poses become fixed
physical rules, including their 30-degree orientations and a 1 mm LED courtyard
edge margin; other components keep the ordinary 2 mm margin. These are not
copper-clearance exemptions.

## Reproduce

From the repository root, with installed KiCad footprints (PowerShell):

```powershell
uv run python -m copperscript check examples/round_led_ring.copper --locked
uv run python -m pcbir.round_led_example `
  --footprint-root "C:\Program Files\KiCad\10.0\share\kicad\footprints" `
  --kicad-cli "C:\Program Files\KiCad\10.0\bin\kicad-cli.exe"
```

Output: ignored `build/round-led-ring/`. The default is a placed, **unrouted**
inspection project with source/local KiCad footprints, front/back placement SVGs,
physical DRC, native KiCad DRC, summary and `profile.pstats`/`profile.txt`. Add
`--offline` after the URL package cache is populated. `--output-dir` changes the
destination. Performance profiling is always enabled.

To attempt package escapes and detailed signal routing with native ground refill:

```powershell
uv run python -m pcbir.round_led_example --route `
  --footprint-root "C:\Program Files\KiCad\10.0\share\kicad\footprints" `
  --kicad-cli "C:\Program Files\KiCad\10.0\bin\kicad-cli.exe" `
  --output-dir build/round-led-ring-routed
```

Package/global access radius is explicitly 5 mm for this example: the holder's
17.8 mm square underside ground contact blocks through-vias beneath the MCU.
Vias must escape that contact, not short through it. Ordinary via/pad overlap
remains forbidden. No via-in-pad permission, hidden routing or clearance waiver
is added. Front GND-pad escapes are reserved **before** signal package escapes
and detailed routing. Ground uses a circularly inset rear fill intent and legal
off-pad contacts, not an indiscriminate field of vias or unnecessary front-pour
islands. The plane-escape owner now has an explicit opt-in for opposite-side
surface zones on two-layer boards; same-side pours do not force extra vias.
Native refill proves actual plane connectivity;
the physical IR intentionally retains only fill intent.

Exit 1 from `--route` means the routing/native gates did not all pass (including
when native verification was not requested). Inspect `summary.json`,
`fanout.json`, `detailed-route.json` and `kicad-drc.json`; do not infer completion
from the presence of a PCB file. Native zero opens/violations is distinct from
manufacturing qualification and from native filled-zone polygons in the IR.
Each rerun resets the summary to `running` before generation, so an interrupted
run cannot reuse a previous successful completion status.
The physical DRC report can still show GND opens because it does not fabricate
filled-zone evidence from an unfilled intent.

### Verified baseline

The placed example passes electrical ERC and placement legality. Native KiCad
10.0.6 reports **zero geometry/library violations and 57 unconnected items**
for this deliberately unrouted baseline. The focused circle/mechanical/plane
regression suite passes 84 tests. The GND-first optional routing attempt was
interrupted after 20 minutes without a completed result; do not treat the
example as fully routed. The large rear battery contact restricts through-via
access beneath the MCU. Further routing/placement tuning is follow-up work,
not a clearance or via/pad-overlap exemption.

## Electrical / firmware assumptions

Supply is an installed, non-rechargeable 3 V nominal CR2032. There is no charger,
external power connector or regulator. SWD pin 1 is **target-voltage sense**:
disable debugger power injection and do not connect an externally powered supply
with the primary cell installed. The MCU uses its internal oscillator. Preserve
SWD on PA13/PA14, normal user-flash boot configuration and brownout protection;
unused pins should be configured for low leakage in firmware. No firmware is
provided.

Each channel is `VBAT -> 10 kohm -> LED anode`, with the cathode on an MCU GPIO.
LOW turns it on; HIGH (or a suitable high-impedance state) turns it off. Drive
ordinary push-pull GPIO, not an accidentally enabled alternate function. The
schematic GPIO map and resistor values are authoritative in the `.copper` file.

The LEDs are existing **generic 0603 references**, not orderable light emitters.
Choose efficient low-current red LEDs and check their exact footprint/polarity,
forward voltage, usable brightness and leakage. For illustration, at 3 V and
Vf=1.8 V, a 10 kohm resistor gives roughly 120 uA before GPIO voltage drop.
Brightness falls with cell voltage. This is a low-duty-cycle indicator/chaser,
not a high-brightness, continuously illuminated ring. Prefer one LED at a time
and sleep the MCU between short low-clock-rate updates; all LEDs plus a busy MCU
can quickly dominate battery drain. No lifetime or battery pulse qualification
is claimed.

[ST specifies a 1.7–3.6 V operating supply for STM32G0C1RE](https://www.st.com/en/microcontrollers-microprocessors/stm32g0c1re.html).
[Panasonic's CR2032 data](https://energy.panasonic.com/na/business/products/lithium/coin-cr-standard/models/CR2032)
lists 0.2 mA continuous drain for its standard specification; that is not a
universal maximum-current rating for every CR2032. Verify discharge, sag,
temperature and average/pulse current against the actual cell and hardware.
The 4.7 uF capacitor does not by itself solve a sustained overload. Keep the
coin cell inaccessible to children in the final enclosure.

## Mechanical implementation and limitations

Electrical connectivity stays in `.copper`; the separate Python builder owns
the physical circle, ring placement and round fill polygons while dedicated
mechanical language/datums are pending. `BoardOutline.circle()` retains an
authoritative `CircularBoardBoundary` and an explicitly bounded inscribed query
ring (default chord error 0.01 mm). Every ring edge is integer/Fraction-certified
inside the circle with that radial bound. Shared material/copper predicates use
the exact disk; grid-only consumers may remain conservatively inside the ring.
KiCad exports one native `gr_circle` on `Edge.Cuts`, never the query facets.

General arc paths, slots, circular cutouts and independent curved-outline CAM /
tooling qualification remain future work. Manufacturing release stays blocked
for this new mechanical geometry, even with a clean native routed board. The
example is not a complete enclosure, production BOM, firmware or production
release. See [mechanical geometry](mechanical-geometry.md).
