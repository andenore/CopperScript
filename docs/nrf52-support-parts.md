# nRF52 demo support parts

Prototype-only support package for CopperScript's CR2032 hard-macro example.
The holder is [Keystone 3034](https://www.keyelco.com/product.cfm/product_id/798),
which accepts a CR2032. Its KiCad footprint has two positive lands numbered 1
and a negative contact numbered 2. The holder is passive; an installed cell,
not the empty holder, supplies energy.
The positive tabs are explicitly internally connected (`internal_pad_groups =
"1"`); solder both for mechanical retention, but no PCB bridge is required.
The negative contact still needs an external connection. See the repository's
[internal-contact generation checklist](../../internal-pad-connectivity.md).

The crystal and LED are explicit **reference placeholders**, not orderable
parts. Crystal oscillator terminals follow KiCad's Crystal_GND24 convention
(1/3 signal, 2/4 case ground). Select and verify an actual 32 MHz, CL=8 pF crystal
against [Nordic's reference circuitry, QFN48 LDO Table 1](https://docs.nordicsemi.com/r/bundle/ps_nrf52832/page/ref_circuitry.html).
The LED follows KiCad's cathode=1, anode=2 convention. Choose a low-current LED
with suitable forward voltage and check brightness over battery discharge.

Production publication is blocked by the unresolved orderable crystal/LED,
component tolerances, antenna tuning, and battery pulse-current performance.
No charging, cell protection, power switching or debug-probe power injection
is implied by these definitions.
