# RFC: Rich device and package semantics

**Status:** Implemented and accepted
**Target:** the unreleased CopperScript electrical IR and source language
**Motivation:** compatibility trials with nRF52840, CYUSB4014 (EZ-USB FX10),
AD4134, OPA2197, and the existing STM32G0 examples

This document records the implemented rich device-model revision. It was
validated against the five original fixture families plus Lattice iCE40 FPGA
I/O and TI ISO6721 isolated-interface structures. No compatibility or migration
layer exists because CopperScript has not had a public schema release.

## 1. Problem statement

The current model works for flat package pins, simple electrical capabilities,
power domains, and explicitly enumerated MCU alternate-function rows. The
compatibility trials uncovered several cases that cannot be expressed cleanly:

- Nordic peripherals can route through PSEL registers to large sets of GPIOs.
  Enumerating every peripheral-signal/pad combination produces an artificial
  Cartesian product.
- FX10 pins have mode-dependent identities and may operate as LVCMOS pins or
  members of LVDS pairs.
- AD4134 inputs are differential analog groups. It also has several supply
  classes, do-not-connect pins, and an exposed pad with a mandatory connection.
- OPA2197 contains two repeated amplifier units with shared supply pins.
- `PartKind` and `InterfaceKind` are closed enums, so every new device class or
  protocol currently requires a core compiler change.
- One `PinCapability` set mixes signal domain, direction, and drive behavior.
  It cannot precisely describe analog inputs, clock pins, references,
  differential polarity, RF pins, or high-impedance states.

The goal is a compact model that preserves these facts without turning
CopperScript into a firmware description language, an analog simulator, or a
schematic drawing format.

## 2. Design principles

1. **Physical truth and semantic views are separate.** Device pads and package
   pins remain physical identities. Functional units, interfaces, and signal
   groups are semantic views that resolve to those identities.
2. **Nets remain the only source of connectivity.** Unit terminals, mux rules,
   modes, and package rules annotate or constrain endpoints; they never create
   net membership.
3. **The core uses orthogonal electrical concepts.** Signal domain, direction,
   drive behavior, grouping, and limits are independent fields rather than an
   ever-growing pin-type enum.
4. **Taxonomies are open; mechanics are typed.** Device categories and
   interface names may be package-qualified strings. Small stable enums are
   appropriate for mechanics such as direction, differential polarity, and
   connection policy.
5. **Selections are explicit and deterministic.** CopperScript records a
   concrete user selection. Rules validate that selection; they do not hide an
   automatic firmware configuration step.
6. **Conditions remain bounded.** The first implementation supports named mode
   choices and conjunctions of equality tests, not a general boolean or SAT
   language.
7. **Backends own presentation.** Units may inform multi-unit schematic
   symbols, but symbol coordinates and graphical grouping never enter the
   electrical IR.

## 3. Proposed IR changes

### 3.1 Open part categories and traits

Replace the closed `PartKind` enum with an open category identifier and an
optional set of traits:

```python
@dataclass(frozen=True, slots=True)
class PartDefinition:
    name: str
    pins: Mapping[str, PackagePinDefinition]
    category: str = "component.generic"
    traits: frozenset[str] = frozenset()
    # existing package, manufacturer, device, source, and metadata fields
```

Examples include `semiconductor.mcu`, `controller.usb`, `converter.adc`, and
`amplifier.opamp`. Standard names should use a `std.` namespace in serialized
IR. Package-qualified names may define experimental categories. ERC must not
attach behavior to an unknown category; checks consume typed fields and traits.

### 3.2 Orthogonal electrical profiles

Replace `PinCapability` as the primary representation with an electrical
profile whose axes can be combined:

```python
class SignalDomain(str, Enum):
    DIGITAL = "digital"
    ANALOG = "analog"
    POWER = "power"
    GROUND = "ground"
    CLOCK = "clock"
    RF = "rf"

class Direction(str, Enum):
    INPUT = "input"
    OUTPUT = "output"
    BIDIRECTIONAL = "bidirectional"
    PASSIVE = "passive"

class DriveMode(str, Enum):
    PUSH_PULL = "push_pull"
    OPEN_DRAIN = "open_drain"
    HIGH_IMPEDANCE = "high_impedance"

@dataclass(frozen=True, slots=True)
class ElectricalProfile:
    domains: frozenset[SignalDomain]
    directions: frozenset[Direction]
    drive_modes: frozenset[DriveMode] = frozenset()
    traits: frozenset[str] = frozenset()
    voltage: QuantityRange[Voltage] | None = None
    current: QuantityRange[Current] | None = None
```

Traits cover extensible semantic facts such as `reference`, `reset`,
`oscillator`, or `clock_output`. A pad may be both analog and digital and may
support input plus push-pull and open-drain output. Power and ground remain
explicit domains rather than descriptive role strings.

`PeripheralSignalDefinition` should state a required profile, not reuse the
physical pin's profile. ERC then checks whether the selected pad satisfies the
requirement.

### 3.3 Functional units and terminals

Add package-independent functional units to `DeviceDefinition`:

```python
@dataclass(frozen=True, slots=True)
class TerminalBinding:
    pad: str
    profile: ElectricalProfile | None = None

@dataclass(frozen=True, slots=True)
class FunctionalUnitDefinition:
    name: str
    kind: str
    terminals: Mapping[str, TerminalBinding]
    shared: bool = False
```

For OPA2197, units `A` and `B` each expose `INP`, `INN`, and `OUT`; a shared
power unit exposes `VP` and `VN`. The bindings resolve each terminal to one
device pad. The part separately bonds package pins to those pads.

Board endpoints may use semantic paths such as `U1.A.OUT`. Lowering resolves
that path to one physical package pin and stores both the semantic origin and
the resolved physical endpoint. ERC's pin-on-multiple-nets rule operates on the
resolved physical identity. `U1.OUTA` may remain a physical-pin spelling, but
both spellings must resolve to the same canonical endpoint.

Simple passives do not need synthetic public units. A `PartDefinition` may
continue to exist without a `DeviceDefinition`.

### 3.4 Signal groups

Add named groups for electrical relationships between signals:

```python
class GroupKind(str, Enum):
    DIFFERENTIAL_PAIR = "differential_pair"

@dataclass(frozen=True, slots=True)
class SignalGroupDefinition:
    name: str
    kind: GroupKind | str
    members: Mapping[str, str]
    profile: ElectricalProfile | None = None
    when: Condition | None = None
```

A differential pair uses members `positive` and `negative`. The same construct
represents AD4134 analog inputs, USB D+/D-, or an FX10 LVDS pair; protocol-specific
meaning belongs in an interface definition. Future group kinds can be open
strings without changing the core representation.

### 3.5 Package bonds and connection policy

Replace bare `bonded_pads: tuple[str, ...]` with typed conditional bonds and a
connection policy:

```python
class ConnectionPolicy(str, Enum):
    NORMAL = "normal"
    REQUIRED = "required"
    DO_NOT_CONNECT = "do_not_connect"
    OPTIONAL = "optional"

@dataclass(frozen=True, slots=True)
class BondDefinition:
    pad: str
    when: Condition | None = None

@dataclass(frozen=True, slots=True)
class PackagePinDefinition:
    name: str
    number: str
    bonds: tuple[BondDefinition, ...] = ()
    profile: ElectricalProfile | None = None
    connection_policy: ConnectionPolicy = ConnectionPolicy.NORMAL
    required_net_traits: frozenset[str] = frozenset()
```

An exposed pad is represented as `REQUIRED` with a required `ground` net
trait. A DNC pin is `DO_NOT_CONNECT`. This is more precise than inferring rules
from names such as `EPAD`, `NC`, or `DNC`.

Several physical pins may bond to one pad, and one package pin may have
conditional alternative bonds. Unconditional existing bonds are the simple
case.

### 3.6 Named modes and conditions

Add finite named mode groups to devices and components:

```python
@dataclass(frozen=True, slots=True)
class ModeGroupDefinition:
    name: str
    choices: tuple[str, ...]
    default: str | None = None

@dataclass(frozen=True, slots=True)
class Condition:
    selections: Mapping[str, str]  # conjunction: group == choice
```

Mode selections may control bond availability, pin profiles, signal groups,
interfaces, or route rules. Each mode group has at most one selected choice.
The compiler rejects an unavailable endpoint rather than silently choosing a
mode.

FX10 can therefore expose an LVCMOS identity in one mode and a positive or
negative LVDS member in another, while retaining one physical package pin.
The first implementation should not support arbitrary negation, disjunction,
numeric expressions, or conditions on net connectivity.

### 3.7 Parametric routing rules

Keep explicit `MuxOption` rows, but make them one form of a more general route
model:

```python
@dataclass(frozen=True, slots=True)
class PadSetDefinition:
    name: str
    pads: tuple[str, ...]

@dataclass(frozen=True, slots=True)
class SelectorScheme:
    kind: str
    parameters: Mapping[str, str] = field(default_factory=dict)

@dataclass(frozen=True, slots=True)
class RouteRule:
    peripheral: str
    signal: str
    pad_set: str
    selector: SelectorScheme
    when: Condition | None = None
```

The compiled IR stores concrete pad sets. Source and generator inputs may offer
range/set syntax, but lowering expands it deterministically. Exclusions and
package limitations must be explicit.

For nRF PSEL, one rule says that `UARTE0.TXD` accepts pads in `GPIO_PSEL`; the
selector scheme derives the register value from the selected port and pin.
ERC validates membership without storing a row for every signal/pad pairing.
STM32 alternate functions remain explicit route options because selector
values differ per pad and signal. Both forms lower to one validation API.

### 3.8 Data-defined interfaces

Replace the closed `InterfaceKind.I2C` enum with package-qualified interface
type names and declarative definitions:

```python
@dataclass(frozen=True, slots=True)
class InterfaceTypeDefinition:
    name: str
    signals: Mapping[str, ElectricalProfile]
    groups: tuple[SignalGroupDefinition, ...] = ()
    validator: str | None = None
```

The standard library should provide `std.i2c`, `std.spi`, `std.uart`,
`std.usb2`, and a generic `std.lvds` profile. Declarative signal requirements
are portable. A small, trusted compiler registry may attach specialized ERC
passes such as I2C pull-up validation by the stable type name. Dependency
packages cannot execute validators.

Interfaces continue to refer to existing nets and endpoints. They do not
configure firmware or create wires.

### 3.9 Typed ranges and ratings

Generalize the existing voltage minimum/maximum fields:

```python
@dataclass(frozen=True, slots=True)
class QuantityRange(Generic[Q]):
    minimum: Q | None = None
    typical: Q | None = None
    maximum: Q | None = None
    rating: str = "operating"  # operating, recommended, absolute
    when: Condition | None = None
```

The quantity system should add current, frequency, impedance, and capacitance
only as required by fixtures. Static typed ranges are sufficient initially.
Supply-relative formulas, transfer functions, gain/error models, timing
simulation, and SPICE behavior are out of scope.

`PowerDomainDefinition` should gain an operating-voltage range and optional
sequencing dependencies. Existing named steady-state power analysis remains a
separate consumer.

### 3.10 Provenance stays outside the semantic burden

`SourceReference` remains optional. Production generator bundles should retain
document-level provenance, while field-level evidence and extraction confidence
remain in generator sidecars. They are audit data, not required electrical
semantics. Generation should still reject unresolved or contradictory evidence.

## 4. Source sketches

These examples illustrate intent; exact grammar is part of the parser phase.

```copper
device OPA2197_DIE {
    pad A_INP { electrical = analog input; }
    pad A_INN { electrical = analog input; }
    pad A_OUT { electrical = analog output; }

    unit A: std.opamp {
        terminal INP = A_INP;
        terminal INN = A_INN;
        terminal OUT = A_OUT;
    }
}

part OPA2197ID {
    device = OPA2197_DIE;
    category = "amplifier.opamp";
    pin OUTA { number = "1"; bond = A_OUT; }
}

net FILTER_OUT { U1.A.OUT; R1.1; }
```

```copper
padset GPIO_PSEL = P0_00 .. P1_15 except { P0_09, P0_10 };
route UARTE0.TXD to GPIO_PSEL using nrf_psel;
route UARTE0.RXD to GPIO_PSEL using nrf_psel;
```

```copper
modegroup P0_MODE { LVCMOS; LVDS; }
group P0D7: differential_pair when P0_MODE == LVDS {
    positive = P0D7P;
    negative = P0D7N;
}
```

```copper
group AIN0: differential_pair {
    positive = AIN0P;
    negative = AIN0N;
}
pin DNC1 { number = "17"; connection = do_not_connect; }
pin EPAD {
    number = "EP";
    connection = required;
    require net_trait ground;
}
```

## 5. ERC consequences

Implement each rule as an independent pass with stable diagnostic codes:

- resolve unit-terminal and physical-pin spellings to one canonical endpoint;
- reject unknown units, terminals, pads, bonds, pad sets, modes, and groups;
- retain the existing one-physical-pin-per-net rule after resolution;
- reject a connection to `DO_NOT_CONNECT` pins;
- require `REQUIRED` and exposed-pad connections and validate required net
  traits;
- require all mandatory differential members, preserve polarity, and diagnose
  a member used under an unavailable mode;
- reject conflicting selections in a mode group;
- validate explicit route options and route-rule pad-set membership;
- compare required and provided electrical profiles;
- validate typed operating ranges against supplies and power domains; and
- continue protocol-specific checks through trusted validators, including I2C
  pull-ups.

No new ERC should attempt to prove firmware scheduling, op-amp stability,
converter performance, signal integrity, or analog correctness. Firmware-managed
sharing remains an explicit ownership waiver, not an electrical waiver.

## 6. Acceptance fixtures

The implementation is accepted because these fixtures compile, serialize
deterministically, and exercise ERC:

### STM32G0

- Existing explicit alternate-function selections still validate.
- Shared resource settings and package-pad availability retain their current
  behavior.

### nRF52840

- PSEL-capable signals validate through pad-set membership without an
  N-signals-by-M-pads mux table.
- A non-candidate or unbonded package pad is rejected.
- The concrete selector value is deterministic and retained in selected IR.

### CYUSB4014 / FX10

- One physical ball can expose the correct LVCMOS or LVDS semantic identity for
  the selected mode.
- Conflicting mode selections are rejected.
- An LVDS group validates pair completeness and polarity.

### AD4134

- Each analog channel is represented as a differential group.
- Connecting a DNC pin is an error.
- The exposed pad is required on an appropriate ground net.
- Distinct supply domains are checked against typed voltage ranges.

### OPA2197

- `U1.A.OUT` and `U1.B.OUT` resolve to distinct physical pins.
- Both units share the same supply terminals without duplicating physical
  connectivity.
- The schematic backend resolves both units' semantic terminals while keeping
  all generated coordinates backend-local. Native multi-unit presentation is a
  separate rendering enhancement.

## 7. Implementation record

The IR, parser/lowering, serializer, generator schema, canonical endpoint
resolver, and structural ERC checks are implemented. The KiCad backend resolves
semantic unit terminals to physical pins while retaining backend-owned
coordinates. Native multi-unit symbol presentation and language-server support
remain backend/editor enhancements rather than electrical-model requirements.

## 8. Accepted decisions

- Functional units belong to `DeviceDefinition`, not `PartDefinition`; packages
  only bond physical pins to device pads.
- Semantic endpoint paths remain in source/provenance, while compiled IR also
  stores the resolved physical identity used by ERC.
- Conditions are limited to equality selections over finite mode groups.
- Unknown category and interface names remain representable. Core behavior is
  never inferred solely from an open taxonomy string.
- Analog checking initially covers structure, connections, polarity, package
  rules, and typed ranges—not behavioral simulation.
- Device-specific facts that do not affect generic checking remain namespaced
  metadata instead of becoming new core enums.
