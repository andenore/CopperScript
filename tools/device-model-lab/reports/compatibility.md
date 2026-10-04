# CopperScript device-model compatibility lab

CopperScript revision: `ee63d69`

All four cases are deliberately non-publishable bounded fixtures.

## nRF52840-QIAA (Nordic Semiconductor)

Package: aQFN73; scope: aQFN73 balls A8/A12/A18/A22/AD4/AD6 and flexible PSEL routing

| Concept | Classification | Finding |
|---|---|---|
| `closed_part_kind` | **represented** | MCU maps to the existing mcu kind. |
| `analog_direction_semantics` | **lossy** | ANALOG capability exists, but mixed digital/analog pin semantics are not typed. |
| `differential_pair_grouping` | **represented** | No differential pin group is required in this bounded slice. |
| `repeated_functional_units` | **represented** | No repeated subunit is selected in this bounded slice. |
| `shared_power_pins` | **lossy** | Repeated VDD/decoupling relationships are not modeled as package power topology. |
| `wildcard_pin_routing` | **expansion-risk** | PSEL routing is parametric rather than an enumerated mux table; risk metric is 48 GPIO candidates per routable signal. |
| `high_speed_interface_semantics` | **lossy** | USB/NFC/RF fixed functions cannot carry their electrical/interface semantics in the current pin model. |
| `no_connect_exposed_pad_rules` | **unrepresentable** | Package-specific fixed-function and connection recommendations lack a rule representation. |
| `voltage_current_range_metadata` | **lossy** | Voltage fields exist on pads, but supply/current/range metadata is not modeled at this scope. |
| `multi_interface_kinds` | **expansion-risk** | The current compiler's high-level interface model is not a general Nordic peripheral interface schema. |

Evidence facts: 9; sources: 3; production publishable: `false`.

Source: [nRF52840 Product Specification](https://docs.nordicsemi.com/r/bundle/ps_nrf52840/page/pin.html) — latest HTML; revision history v1.11; aQFN73 ball assignments, Table 1
Source: [nRF52840 Product Specification](https://docs.nordicsemi.com/r/bundle/ps_nrf52840/page/ordering_info.html) — latest HTML; revision history v1.11; Package variant codes, Table 2; QIAA aQFN73
Source: [nRF52840 Product Specification revision history](https://docs.nordicsemi.com/r/bundle/ps_nrf52840/page/rev_history.html) — v1.11; revision history

## CYUSB4014-FCAXI (Infineon/Cypress)

Package: 169-ball FBGA; scope: P0D7P/P0D7N and P0D6P/P0D6N LVDS pairs with LVCMOS aliases, plus P4.3, USB and VDDIO_P0 concepts

| Concept | Classification | Finding |
|---|---|---|
| `closed_part_kind` | **lossy** | MCU is the nearest kind, but FX10 is a USB/high-bandwidth controller with distinct semantics. |
| `analog_direction_semantics` | **represented** | No analog pin is selected in this bounded slice. |
| `differential_pair_grouping` | **unrepresentable** | Positive/negative LVDS identities and pair association are absent from pads and mux rows. |
| `repeated_functional_units` | **expansion-risk** | Seven SCBs and repeated port structures require subunit identity; bounded mode variants already multiply rows. |
| `shared_power_pins` | **lossy** | VDDIO domain association can be approximated, but mode-dependent supply constraints are not modeled. |
| `wildcard_pin_routing` | **expansion-risk** | Shared IO matrix and mode-dependent names cannot be represented without enumerating combinations; two electrical modes occur per selected data signal. |
| `high_speed_interface_semantics` | **unrepresentable** | USB HS/USB 3.2/LVDS electrical interface classes exceed the current signal type model. |
| `no_connect_exposed_pad_rules` | **lossy** | No bounded DNC/EP fact selected, but package rules are not first-class. |
| `voltage_current_range_metadata` | **lossy** | Supply domains exist in source facts, while current and mode-specific electrical ranges do not fit the bundle. |
| `multi_interface_kinds` | **unrepresentable** | I2C/UART/SPI/CAN/I2S/USB and GPIF are not represented as typed CopperScript interfaces. |

Evidence facts: 7; sources: 1; production publishable: `false`.

Source: [CYUSB401x EZ-USB FX10 USB 10 Gbps peripheral controller datasheet](https://www.infineon.com/dgdl/Infineon-CYUSB401x_EZ-USB_FX10_USB_10_Gbps_peripheral_controller-DataSheet-v02_00-EN.pdf?fileId=8ac78c8c956a0a4701959618c62d4947) — 002-40837 Rev. *C, 2026-03-10; package information p. 2; pin definitions Table 9 p. 31 and Table 11 pp. 35-36; ordering information p. 82

## AD4134BCPZ (Analog Devices)

Package: 56-lead LFCSP with EP; scope: AIN0-3 differential pairs, reference pins, supplies, clocks, SPI/pin-control and DOUT concepts

| Concept | Classification | Finding |
|---|---|---|
| `closed_part_kind` | **unrepresentable** | No ADC/data-converter PartKind exists; generic would discard the device class. |
| `analog_direction_semantics` | **unrepresentable** | AI/AO/DI/O and analog channel semantics do not map to the current typed peripheral/pin model. |
| `differential_pair_grouping` | **unrepresentable** | AINx+ and AINx− polarity and channel grouping have no representation. |
| `repeated_functional_units` | **expansion-risk** | Four ADC channels and multiple DOUT outputs need repeated subunits; 4 channels × 2 polarities is the bounded endpoint count. |
| `shared_power_pins` | **lossy** | Multiple analog/digital/clock rails can be listed, but internal LDO and ground relationships are not modeled. |
| `wildcard_pin_routing` | **expansion-risk** | SPI versus pin-control and serialized versus parallel output modes are configuration-dependent. |
| `high_speed_interface_semantics` | **lossy** | 48 MHz clock and high-rate data timing are outside the current interface model. |
| `no_connect_exposed_pad_rules` | **unrepresentable** | DNC pin and exposed-pad grounding rules are not first-class package constraints. |
| `voltage_current_range_metadata` | **lossy** | Multiple exact rail ranges are documented, but bundle metadata has no complete per-device supply/range schema. |
| `multi_interface_kinds` | **unrepresentable** | SPI, pin-control, DOUT and clock/control interfaces exceed the current I2C-centered interface model. |

Evidence facts: 8; sources: 1; production publishable: `false`.

Source: [AD4134 datasheet](https://www.analog.com/media/en/technical-documentation/data-sheets/ad4134.pdf) — Rev. 0, 2021; Table 8, Pin Function Descriptions, pp. 14-16; ordering guide p. 92; SPI/data interface pp. 52, 62-64

## OPA2197ID (Texas Instruments)

Package: SOIC-8 (D); scope: two amplifier units, shared V+/V− pins, analog input/output direction and supply range

| Concept | Classification | Finding |
|---|---|---|
| `closed_part_kind` | **unrepresentable** | No operational-amplifier PartKind exists. |
| `analog_direction_semantics` | **lossy** | Current input/output types do not express analog voltage semantics. |
| `differential_pair_grouping` | **expansion-risk** | Each amplifier has a +IN/−IN pair, but the pair-to-unit relationship is not modeled. |
| `repeated_functional_units` | **unrepresentable** | OPA2197 units A and B require repeated subunits; current parts have one flat pin namespace. |
| `shared_power_pins` | **lossy** | Shared V+ and V− pins cannot be associated with both functional units. |
| `wildcard_pin_routing` | **represented** | No configurable pin routing is selected in this fixed-function package. |
| `high_speed_interface_semantics` | **represented** | No high-speed digital interface is selected in this bounded slice. |
| `no_connect_exposed_pad_rules` | **represented** | No DNC or exposed-pad rule is selected in the OPA2197 SOIC-8 slice. |
| `voltage_current_range_metadata` | **lossy** | Supply range is documented, but current model lacks complete device-level range metadata. |
| `multi_interface_kinds` | **represented** | No digital interface is selected in this bounded slice. |

Evidence facts: 7; sources: 1; production publishable: `false`.

Source: [OPAx197 36-V Precision Rail-to-Rail Input/Output Low Offset Voltage Operational Amplifiers datasheet](https://www.ti.com/lit/ds/symlink/opa2197.pdf) — SBOS737C, revised March 2018; Pin Configuration and Functions pp. 2-3; Recommended Operating Conditions p. 5; package addendum pp. 34, 38-39
