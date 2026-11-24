//! Stable loop identity and passive port conventions shared by the numerical adapters.
//!
//! A pad count is not a topology. Named four-switch loops deliberately remain unsupported
//! by the buck port builder until their terminal groups and return paths are validated.

use copper_json::{Object, Value};

use super::EmiFinding;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopKind {
    #[default]
    Standard,
    Input,
    Output,
}

impl LoopKind {
    pub fn value(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Input => "input",
            Self::Output => "output",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "standard" => Ok(Self::Standard),
            "input" => Ok(Self::Input),
            "output" => Ok(Self::Output),
            _ => Err(format!("unknown loop kind {value:?}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopId {
    pub controller: String,
    pub kind: LoopKind,
}

impl LoopId {
    /// Read the structured identity. Only exact historical names are accepted as a fallback;
    /// malformed structured data must never fall back to a different loop.
    pub fn from_finding(finding: &EmiFinding) -> Result<Self, String> {
        let controller = finding.cell.as_deref().filter(|s| !s.is_empty()).ok_or("loop has no controller")?;
        let kind = if let Some(value) = finding.measures.get("loop_id") {
            let object = value.as_object().ok_or("loop_id must be an object")?;
            if object.get("controller").and_then(Value::as_str) != Some(controller) {
                return Err("loop_id controller does not match the finding's cell".to_owned());
            }
            LoopKind::parse(object.get("kind").and_then(Value::as_str).ok_or("loop_id has no kind")?)?
        } else {
            match finding.measures.get("loop") {
                None => LoopKind::Standard,
                Some(Value::Str(name)) if name == "input loop" => LoopKind::Input,
                Some(Value::Str(name)) if name == "output loop" => LoopKind::Output,
                Some(_) => return Err("unknown legacy loop name; a structured loop_id is required".to_owned()),
            }
        };
        Ok(Self { controller: controller.to_owned(), kind })
    }

    /// Preserve the historical single-loop buck key, with stable suffixes for named loops.
    pub fn key(&self) -> String {
        match self.kind {
            LoopKind::Standard => self.controller.clone(),
            other => format!("{}:{}", self.controller, other.value()),
        }
    }

    pub fn to_value(&self) -> Value {
        let mut value = Object::new();
        value.insert("controller".to_owned(), Value::from(self.controller.as_str()));
        value.insert("kind".to_owned(), Value::from(self.kind.value()));
        Value::Object(value)
    }
}

/// A selected physical pad of the screening contour, not an equipotential terminal group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopPad {
    pub component: String,
    pub pad: String,
}

impl LoopPad {
    pub fn from_value(value: &Value) -> Result<Self, String> {
        let field = |key| {
            value
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("loop pad has no nonempty {key}"))
        };
        Ok(Self { component: field("component")?, pad: field("pad")? })
    }

    pub fn to_value(&self) -> Value {
        let mut value = Object::new();
        value.insert("component".to_owned(), Value::from(self.component.as_str()));
        value.insert("pad".to_owned(), Value::from(self.pad.as_str()));
        Value::Object(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosureKind {
    Capacitor,
    CatchDiode,
    LowSideSwitch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopClosure {
    pub terminals: (usize, usize),
    pub kind: ClosureKind,
}

/// Indices into the contour's pads, with the optional switch pad appended at index four.
/// Closures are passive extraction fixtures, not simultaneous transistor operating states.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopPortLayout {
    pub port: (usize, usize),
    pub closures: Vec<LoopClosure>,
    pub legs: Vec<(usize, usize)>,
    pub append_switch_pad: bool,
}

/// A single-capacitor PCB contour, excited at the controller's rail/return pads.
/// This does not assert any package-internal path or transistor operating state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalContour {
    pub id: LoopId,
    pub pads: [LoopPad; 4],
    /// Separate physical contacts; never an equipotential group across package pins.
    pub groups: [Vec<LoopPad>; 4],
    /// Number of footprint copper primitives for each pad identity. Repeated
    /// numbers may describe one continuous land; the geometry adapter must prove it.
    pub occurrences: Vec<(LoopPad, usize)>,
    pub capacitors: Vec<String>,
    pub provenance: String,
}

impl ExternalContour {
    pub fn from_finding(finding: &EmiFinding) -> Result<Self, String> {
        if !finding.measures.contains_key("loop_id") {
            return Err("external contour requires structured loop identity and terminal groups".to_owned());
        }
        let id = LoopId::from_finding(finding)?;
        if id.kind == LoopKind::Standard {
            return Err("external buck-boost contour requires an input or output identity".to_owned());
        }
        let groups = finding
            .measures
            .get("loop_terminal_groups")
            .and_then(Value::as_array)
            .filter(|g| g.len() == 4)
            .ok_or("external contour requires four structured terminal groups")?;
        let mut parsed = Vec::new();
        let mut occurrences = Vec::new();
        for (index, group) in groups.iter().enumerate() {
            let contacts = group.as_array().ok_or("terminal group must be an array")?;
            if contacts.is_empty() {
                return Err(format!(
                    "terminal group {index} has {} physical contacts; capacitor groups require one contact and controller groups must be nonempty",
                    contacts.len()
                ));
            }
            let mut contacts = contacts.iter().map(LoopPad::from_value).collect::<Result<Vec<_>, _>>()?;
            contacts.sort_by(|a, b| (&a.component, &a.pad).cmp(&(&b.component, &b.pad)));
            let all = contacts.clone();
            contacts.dedup();
            occurrences.extend(contacts.iter().map(|pad| (pad.clone(), all.iter().filter(|p| *p == pad).count())));
            parsed.push(contacts);
        }
        let groups: [Vec<LoopPad>; 4] = parsed.try_into().expect("four groups");
        let selected = finding
            .measures
            .get("loop_pads")
            .and_then(Value::as_array)
            .ok_or("external contour requires structured selected pads")?;
        let selected = selected.iter().map(LoopPad::from_value).collect::<Result<Vec<_>, _>>()?;
        let pads: [LoopPad; 4] = selected.try_into().map_err(|_| "external contour requires four selected pads")?;
        let cap = finding.measures.get("loop_capacitor").and_then(Value::as_str).ok_or("no selected capacitor")?;
        if pads[0].component != cap
            || pads[3].component != cap
            || cap == id.controller
            || pads[1].component != id.controller
            || pads[2].component != id.controller
        {
            return Err(
                "external contour must be capacitor rail, controller rail, controller return, capacitor return"
                    .to_owned(),
            );
        }
        let capacitors = finding
            .measures
            .get("loop_capacitors")
            .and_then(Value::as_array)
            .ok_or("external contour requires the capacitor candidate list")?
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| "invalid capacitor candidate".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !capacitors.iter().any(|c| c == cap) {
            return Err("selected capacitor is absent from the candidate list".to_owned());
        }
        let provenance = finding
            .measures
            .get("loop_terminal_provenance")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("external contour requires terminal provenance")?
            .to_owned();
        let contour = Self { id, pads, groups, occurrences, capacitors, provenance };
        contour.validate()?;
        Ok(contour)
    }

    pub fn validate(&self) -> Result<(), String> {
        let contacts: Vec<_> = self.groups.iter().flatten().collect();
        if contacts.iter().enumerate().any(|(i, p)| contacts[..i].contains(p)) {
            return Err("external contour has duplicate physical contact identities".to_owned());
        }
        for (i, group) in self.groups.iter().enumerate() {
            let component = if i == 0 || i == 3 { &self.pads[0].component } else { &self.id.controller };
            if group.is_empty()
                || ((i == 0 || i == 3) && group.len() != 1)
                || !group.contains(&self.pads[i])
                || group.iter().any(|p| &p.component != component)
            {
                return Err("external contour groups do not match selected capacitor/controller contacts".to_owned());
            }
        }
        // Each extra contact adds a matrix column and a unit-current solution. Bound
        // this independently of the filament budget before any geometry is built.
        if self.groups[1].len() + self.groups[2].len() > 17 {
            return Err("external contour exceeds the 16 contact-port budget".to_owned());
        }
        if self.occurrences.len() != contacts.len()
            || contacts.iter().any(|pad| self.occurrences.iter().filter(|(p, n)| p == *pad && *n > 0).count() != 1)
        {
            return Err("external contour requires an occurrence count for every contact".to_owned());
        }
        Ok(())
    }

    pub fn occurrence_count(&self, component: &str, pad: &str) -> usize {
        self.occurrences.iter().find(|(p, _)| p.component == component && p.pad == pad).map_or(0, |(_, n)| *n)
    }

    pub fn grouped(&self) -> bool {
        self.groups[1].len() > 1 || self.groups[2].len() > 1
    }

    /// Keep the selected four-pad convention, then append each other contact once.
    pub fn contacts(&self) -> Vec<LoopPad> {
        let mut pads = self.pads.to_vec();
        for group in [&self.groups[1], &self.groups[2]] {
            let mut rest: Vec<_> = group.iter().filter(|p| !self.pads.contains(p)).cloned().collect();
            rest.sort_by(|a, b| (&a.component, &a.pad).cmp(&(&b.component, &b.pad)));
            pads.extend(rest);
        }
        pads
    }

    /// Complete N-1 basis relative to one return contact. Every other contact
    /// gets an independent port, including additional returns. No pin short is added.
    pub fn contact_ports(&self) -> Vec<(usize, usize)> {
        let mut ports = vec![(1, 2)];
        ports.extend((4..self.contacts().len()).map(|i| (i, 2)));
        ports
    }

    pub fn layout(&self) -> LoopPortLayout {
        let contacts = self.contacts();
        let mut legs = vec![(0, 1), (2, 3)];
        for (i, contact) in contacts.iter().enumerate().skip(4) {
            legs.push(if self.groups[1].contains(contact) { (0, i) } else { (i, 3) });
        }
        LoopPortLayout {
            port: (1, 2),
            closures: vec![LoopClosure { terminals: (3, 0), kind: ClosureKind::Capacitor }],
            legs,
            append_switch_pad: false,
        }
    }

    pub fn scope_value(&self) -> Value {
        let mut value = Object::new();
        value.insert("loop_id".to_owned(), self.id.to_value());
        value.insert(
            "model".to_owned(),
            Value::from(if self.grouped() { "external_contact_network/v1" } else { "external_rail_return/v1" }),
        );
        value.insert(
            "scope".to_owned(),
            Value::from(if self.grouped() {
                "passive_single_capacitor_contact_network"
            } else {
                "passive_single_capacitor_contour"
            }),
        );
        value.insert(
            "port_convention".to_owned(),
            Value::from(
                "positive at controller rail; negative at power return; facing pad patches, no package gap conductor",
            ),
        );
        value.insert("positive".to_owned(), self.pads[1].to_value());
        value.insert("negative".to_owned(), self.pads[2].to_value());
        value.insert("repeated_pad_numbers".to_owned(), Value::from("allowed only when the declared occurrence count matches one continuous SMD copper land on one layer and net; disconnected same-number lands are unsupported"));
        if self.grouped() {
            let contacts = self.contacts();
            value.insert("contact_port_limit".to_owned(), Value::from(16_i64));
            value.insert(
                "contact_boundary_assumption".to_owned(),
                Value::from(
                    "each individual pad is equipotential; package spreading and contact impedance are excluded",
                ),
            );
            value.insert("port_convention".to_owned(), Value::from("independent whole-pad contacts; N-1 ports referenced to the selected power-return contact; no inter-contact or package shorts"));
            value.insert(
                "ports".to_owned(),
                Value::Array(
                    self.contact_ports()
                        .iter()
                        .map(|&(a, b)| {
                            let mut port = Object::new();
                            port.insert("positive".to_owned(), contacts[a].to_value());
                            port.insert("negative".to_owned(), contacts[b].to_value());
                            Value::Object(port)
                        })
                        .collect(),
                ),
            );
            value.insert(
                "contact_current_sharing".to_owned(),
                Value::from("unspecified; requires package termination or explicit contact currents"),
            );
            value.insert("scalar_and_current_map".to_owned(), Value::from("port 0 unit-current response only; all other port currents zero; not aggregate converter impedance"));
        }
        value.insert("terminal_provenance".to_owned(), Value::from(self.provenance.as_str()));
        value.insert("selected_capacitor".to_owned(), Value::from(self.pads[0].component.as_str()));
        value.insert(
            "other_capacitors_open".to_owned(),
            Value::Array(
                self.capacitors
                    .iter()
                    .filter(|c| **c != self.pads[0].component)
                    .map(|c| Value::from(c.as_str()))
                    .collect(),
            ),
        );
        value.insert(
            "other_loop_boundary".to_owned(),
            Value::from("no excitation or component closure; shared return copper retained within the window"),
        );
        value.insert("inter_loop_coupling".to_owned(), Value::from("not_evaluated"));
        value.insert("excitation".to_owned(), Value::from("unit_port_current_only; converter waveform not modelled"));
        value.insert("coverage".to_owned(), Value::from("partial"));
        value.insert("requested".to_owned(), Value::Bool(true));
        value.insert("executed".to_owned(), Value::Bool(false));
        value.insert("usable".to_owned(), Value::Bool(false));
        Value::Object(value)
    }
}

impl LoopPortLayout {
    pub fn buck(topology: &str, kind: LoopKind, pad_count: usize, switch_pad: bool) -> Result<Self, String> {
        if topology != "buck" || kind != LoopKind::Standard {
            return Err(format!(
                "the {} loop of topology {topology:?} has no validated port definition; only standard buck loops are supported",
                kind.value()
            ));
        }
        let closure = |a, b, kind| LoopClosure { terminals: (a, b), kind };
        match pad_count {
            6 => Ok(Self {
                port: (1, 2),
                closures: vec![closure(3, 4, ClosureKind::CatchDiode), closure(5, 0, ClosureKind::Capacitor)],
                legs: vec![(0, 1), (2, 3), (4, 5)],
                append_switch_pad: false,
            }),
            4 if switch_pad => Ok(Self {
                port: (1, 4),
                closures: vec![closure(4, 2, ClosureKind::LowSideSwitch), closure(3, 0, ClosureKind::Capacitor)],
                legs: vec![(0, 1), (2, 3)],
                append_switch_pad: true,
            }),
            4 => Ok(Self {
                port: (1, 2),
                closures: vec![closure(3, 0, ClosureKind::Capacitor)],
                legs: vec![(0, 1), (2, 3)],
                append_switch_pad: false,
            }),
            n => Err(format!("a hot loop of {n} pads has no port definition (a buck loop has 4 or 6)")),
        }
    }
}

/// One board window and one complete contact-current basis for one or both sides.
/// Shared returns appear once; capacitor fixtures and native legs retain their side.
#[derive(Clone, Debug)]
pub struct ExternalNetwork {
    pub contours: Vec<ExternalContour>,
    pub contacts: Vec<LoopPad>,
    pub ports: Vec<(usize, usize)>,
    pub layout: LoopPortLayout,
}

impl ExternalNetwork {
    pub fn new(primary: ExternalContour, peer: Option<ExternalContour>) -> Result<Self, String> {
        primary.validate()?;
        let mut contours = vec![primary];
        if let Some(peer) = peer {
            peer.validate()?;
            if peer.id.controller != contours[0].id.controller
                || !matches!(
                    (contours[0].id.kind, peer.id.kind),
                    (LoopKind::Input, LoopKind::Output) | (LoopKind::Output, LoopKind::Input)
                )
            {
                return Err("joint extraction requires the input and output of the same controller".to_owned());
            }
            contours.push(peer);
        }
        let mut contacts = Vec::new();
        for contour in &contours {
            for pad in contour.contacts() {
                if !contacts.contains(&pad) {
                    contacts.push(pad);
                }
            }
        }
        let index = |pad: &LoopPad| contacts.iter().position(|p| p == pad).expect("contact collected");
        for pad in &contacts {
            let counts: Vec<_> =
                contours.iter().map(|c| c.occurrence_count(&pad.component, &pad.pad)).filter(|n| *n > 0).collect();
            if counts.iter().any(|n| *n != counts[0]) {
                return Err("joint contours disagree on a contact's occurrence count".to_owned());
            }
        }
        let reference = index(&contours[0].pads[2]);
        let mut ports = vec![(index(&contours[0].pads[1]), reference)];
        for (i, pad) in contacts.iter().enumerate() {
            if pad.component == contours[0].id.controller && i != reference && !ports.iter().any(|p| p.0 == i) {
                ports.push((i, reference));
            }
        }
        // Use each loop's own selected return for its rail contacts. Remaining
        // return contacts keep ports to the primary reference, so this is still
        // a connected N-1 tree basis, not an ideal short between return pins.
        for contour in &contours {
            for rail in &contour.groups[1] {
                let positive = index(rail);
                if let Some(port) = ports.iter_mut().find(|p| p.0 == positive) {
                    port.1 = index(&contour.pads[2]);
                }
            }
        }
        if ports.len() > 16 {
            return Err("joint network exceeds the 16 contact-port budget".to_owned());
        }
        let mut closures = Vec::new();
        let mut legs = Vec::new();
        for contour in &contours {
            let local = contour.contacts();
            let layout = contour.layout();
            for c in layout.closures {
                closures.push(LoopClosure {
                    terminals: (index(&local[c.terminals.0]), index(&local[c.terminals.1])),
                    kind: c.kind,
                });
            }
            legs.extend(layout.legs.iter().map(|&(a, b)| (index(&local[a]), index(&local[b]))));
        }
        let layout = LoopPortLayout { port: ports[0], closures, legs, append_switch_pad: false };
        Ok(Self { contours, contacts, ports, layout })
    }

    pub fn multi_contact(&self) -> bool {
        self.ports.len() > 1
    }

    pub fn occurrence_count(&self, component: &str, pad: &str) -> usize {
        self.contours.iter().map(|c| c.occurrence_count(component, pad)).max().unwrap_or(0)
    }

    pub fn scope_value(&self) -> Value {
        let mut scope = self.contours[0].scope_value().as_object().expect("scope object").clone();
        scope.insert(
            "capacitor_boundary".to_owned(),
            Value::from("copper fixtures between selected capacitor lands; excludes component ESR/ESL"),
        );
        if self.contours.len() == 2 {
            scope.shift_remove("selected_capacitor");
            scope.shift_remove("terminal_provenance");
            scope.insert(
                "terminal_provenance_by_loop".to_owned(),
                Value::Object(self.contours.iter().map(|c| (c.id.key(), Value::from(c.provenance.as_str()))).collect()),
            );
            let selected: Vec<_> = self.contours.iter().map(|c| &c.pads[0].component).collect();
            let open: std::collections::BTreeSet<_> =
                self.contours.iter().flat_map(|c| &c.capacitors).filter(|c| !selected.contains(c)).collect();
            scope.insert(
                "other_capacitors_open".to_owned(),
                Value::Array(open.into_iter().map(|c| Value::from(c.as_str())).collect()),
            );
            scope.insert(
                "contact_current_sharing".to_owned(),
                Value::from("unspecified; requires package termination or explicit contact currents"),
            );
            scope.insert("model".to_owned(), Value::from("external_joint_contact_network/v1"));
            scope.insert("scope".to_owned(), Value::from("passive_two_rail_contact_network"));
            scope.insert("inter_loop_coupling".to_owned(), Value::from("requested_joint_matrix"));
            scope.insert(
                "other_loop_boundary".to_owned(),
                Value::from("both selected capacitors have fixtures; other components open"),
            );
            scope.insert("loops".to_owned(), Value::Array(self.contours.iter().map(|c| c.id.to_value()).collect()));
            scope.insert(
                "selected_capacitors".to_owned(),
                Value::Array(self.contours.iter().map(|c| Value::from(c.pads[0].component.as_str())).collect()),
            );
            scope.insert("port_convention".to_owned(), Value::from("independent whole-pad contact tree; each rail uses its loop's selected return; extra return ports reference the primary return; signed terminal currents follow the listed port pairs; no package shorts"));
            scope.insert(
                "ports".to_owned(),
                Value::Array(
                    self.ports
                        .iter()
                        .map(|&(a, b)| {
                            let mut port = Object::new();
                            port.insert("positive".to_owned(), self.contacts[a].to_value());
                            port.insert("negative".to_owned(), self.contacts[b].to_value());
                            Value::Object(port)
                        })
                        .collect(),
                ),
            );
        }
        Value::Object(scope)
    }
}
