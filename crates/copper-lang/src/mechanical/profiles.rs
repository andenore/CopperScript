//! Declarative board-profile composition and explicit role binding
//! (`pcbir.mechanical_profiles`).

use indexmap::{IndexMap, IndexSet};

use copper_json::python_str_repr as repr;

use crate::compiler::MechanicalProfileDefinition;
use crate::error::CopperScriptError;
use crate::syntax::{Location, MechanicalItem, MechanicalItemDecl, MechanicalProfileUseDecl, Scalar, Value};

/// `MechanicalProfileInstance`: one profile use, by instance path.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalProfileInstance {
    pub name: String,
    pub profile: String,
    pub location: Location,
    pub bindings: Vec<(String, String)>,
}

/// `MechanicalFeatureSource`: the owner of one expanded feature.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalFeatureSource {
    pub kind: String,
    pub name: String,
    pub location: Location,
    pub profile: Option<String>,
    pub instance: Option<String>,
}

const ROLE_KINDS: [&str; 5] = ["connector", "attach", "overhang", "component_height", "assembly_access"];

type Result<T> = std::result::Result<T, CopperScriptError>;

fn error<T>(message: String, location: &Location) -> Result<T> {
    Err(CopperScriptError::new("MEC004", message, location.clone()))
}

fn binding_text(value: &Scalar) -> Option<&str> {
    match value {
        Scalar::Str(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

struct Expansion<'a> {
    profiles: &'a IndexMap<String, MechanicalProfileDefinition>,
    result: Vec<MechanicalItemDecl>,
    sources: Vec<MechanicalFeatureSource>,
    instances: Vec<MechanicalProfileInstance>,
}

impl<'a> Expansion<'a> {
    fn definition(
        &self,
        usage: &MechanicalProfileUseDecl,
        stack: &[String],
    ) -> Result<&'a MechanicalProfileDefinition> {
        let Some(definition) = self.profiles.get(&usage.profile) else {
            return error(format!("unknown board profile {}", repr(&usage.profile)), &usage.location);
        };
        if stack.contains(&usage.profile) {
            let mut cycle: Vec<&str> = stack.iter().map(String::as_str).collect();
            cycle.push(&usage.profile);
            return error(format!("cyclic board profile use: {}", cycle.join(" -> ")), &usage.location);
        }
        Ok(definition)
    }

    /// `roles`: the connector roles a profile use must bind.
    fn roles(&self, usage: &MechanicalProfileUseDecl, stack: &[String]) -> Result<IndexSet<String>> {
        let definition = self.definition(usage, stack)?;
        let mut required: Vec<String> = Vec::new();
        let inner: Vec<String> = stack.iter().cloned().chain([usage.profile.clone()]).collect();
        for item in &definition.items {
            match item {
                MechanicalItem::ProfileUse(child_use) => {
                    let child = self.roles(child_use, &inner)?;
                    let keys: IndexSet<&str> = child_use.bindings.keys().map(String::as_str).collect();
                    let same = keys.len() == child.len() && child.iter().all(|role| keys.contains(role.as_str()));
                    if !same || child_use.bindings.values().any(|value| binding_text(value).is_none()) {
                        return error(
                            "nested profile bindings must map every child role to an outer role".to_owned(),
                            &child_use.location,
                        );
                    }
                    required.extend(child_use.bindings.values().filter_map(binding_text).map(str::to_owned));
                }
                MechanicalItem::Feature(feature) if ROLE_KINDS.contains(&feature.kind.as_str()) => {
                    required.push(feature.name.clone());
                }
                _ => {}
            }
        }
        let unique: IndexSet<String> = required.iter().cloned().collect();
        if unique.len() != required.len() {
            return error("profile contains duplicate connector roles".to_owned(), &definition.location);
        }
        Ok(unique)
    }

    fn append(
        &mut self,
        item: &MechanicalItemDecl,
        profile: Option<&str>,
        path: Option<&str>,
        binding: Option<&Scalar>,
    ) -> Result<()> {
        let name = match path {
            Some(path) if !path.is_empty() && !item.name.is_empty() => format!("{path}/{}", item.name),
            _ => item.name.clone(),
        };
        let mut parameters = item.parameters.clone();
        if ROLE_KINDS.contains(&item.kind.as_str()) && profile.is_some() {
            if parameters.contains_key("component") {
                return error(
                    "profile connector components must be supplied through explicit bindings".to_owned(),
                    &item.location,
                );
            }
            let bound = binding.cloned().unwrap_or(Scalar::Str(String::new()));
            parameters.insert("component".to_owned(), Value::Scalar(bound));
        }
        if let Some(path) = path.filter(|path| !path.is_empty()) {
            for key in ["relative_to", "target", "edge"] {
                if let Some(value) = parameters.get_mut(key) {
                    let Value::Scalar(Scalar::Str(target)) = value else {
                        return error("profile mechanical targets must be named references".to_owned(), &item.location);
                    };
                    *target = format!("{path}/{target}");
                }
            }
        }
        self.result.push(MechanicalItemDecl { name: name.clone(), parameters, ..item.clone() });
        self.sources.push(MechanicalFeatureSource {
            kind: item.kind.clone(),
            name,
            location: item.location.clone(),
            profile: profile.map(str::to_owned),
            instance: path.map(str::to_owned),
        });
        Ok(())
    }

    fn expand(
        &mut self,
        usage: &MechanicalProfileUseDecl,
        path: &str,
        bindings: &IndexMap<String, Scalar>,
        stack: &[String],
    ) -> Result<()> {
        let definition = self.definition(usage, stack)?;
        let required = self.roles(usage, stack)?;
        let exact = bindings.len() == required.len() && required.iter().all(|role| bindings.contains_key(role));
        if !exact || bindings.values().any(|value| binding_text(value).is_none()) {
            let mut roles: Vec<&str> = required.iter().map(String::as_str).collect();
            roles.sort_unstable();
            let names = if roles.is_empty() { "(none)".to_owned() } else { roles.join(", ") };
            return error(
                format!("profile {} requires exactly these connector bindings: {names}", repr(&usage.profile)),
                &usage.location,
            );
        }
        let mut sorted: Vec<(String, String)> = bindings
            .iter()
            .map(|(key, value)| (key.clone(), binding_text(value).unwrap_or_default().to_owned()))
            .collect();
        sorted.sort();
        self.instances.push(MechanicalProfileInstance {
            name: path.to_owned(),
            profile: definition.name.clone(),
            location: usage.location.clone(),
            bindings: sorted,
        });
        let inner: Vec<String> = stack.iter().cloned().chain([usage.profile.clone()]).collect();
        let mut child_names: IndexSet<&str> = IndexSet::new();
        for item in &definition.items {
            match item {
                MechanicalItem::ProfileUse(child) => {
                    if !child_names.insert(&child.instance) {
                        return error("duplicate profile instance name".to_owned(), &child.location);
                    }
                    let mapped: IndexMap<String, Scalar> = child
                        .bindings
                        .iter()
                        .map(|(key, value)| {
                            let outer = binding_text(value).and_then(|role| bindings.get(role)).cloned();
                            (key.clone(), outer.unwrap_or(Scalar::Str(String::new())))
                        })
                        .collect();
                    self.expand(child, &format!("{path}/{}", child.instance), &mapped, &inner)?;
                }
                MechanicalItem::Feature(feature) => {
                    let binding = bindings.get(&feature.name);
                    self.append(feature, Some(&definition.name), Some(path), binding)?;
                }
                MechanicalItem::Stackup(_) => {}
            }
        }
        Ok(())
    }
}

/// `expand_mechanical_items`: expanded items, their owners and profile instances.
pub fn expand_mechanical_items(
    items: &[&MechanicalItem],
    profiles: &IndexMap<String, MechanicalProfileDefinition>,
) -> Result<(Vec<MechanicalItemDecl>, Vec<MechanicalFeatureSource>, Vec<MechanicalProfileInstance>)> {
    let mut expansion = Expansion { profiles, result: Vec::new(), sources: Vec::new(), instances: Vec::new() };
    let mut names: IndexSet<&str> = IndexSet::new();
    for item in items {
        match item {
            MechanicalItem::ProfileUse(usage) => {
                if !names.insert(&usage.instance) {
                    return error("duplicate profile instance name".to_owned(), &usage.location);
                }
                expansion.expand(usage, &usage.instance, &usage.bindings, &[])?;
            }
            MechanicalItem::Feature(feature) => expansion.append(feature, None, None, None)?,
            MechanicalItem::Stackup(_) => {}
        }
    }
    Ok((expansion.result, expansion.sources, expansion.instances))
}
