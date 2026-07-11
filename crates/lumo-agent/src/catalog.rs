use std::collections::BTreeMap;
use std::sync::Arc;

use thiserror::Error;

use crate::{AgentProfile, CapabilityDescriptor};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CapabilityCatalogError {
    #[error("duplicate capability id `{0}`")]
    DuplicateId(String),
    #[error("capability `{0}` has an invalid version hash")]
    InvalidVersionHash(String),
}

pub type CatalogError = CapabilityCatalogError;

#[derive(Debug, Clone, Default)]
pub struct CapabilityCatalogBuilder {
    descriptors: Vec<CapabilityDescriptor>,
}

impl CapabilityCatalogBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn flows(mut self, descriptors: impl IntoIterator<Item = CapabilityDescriptor>) -> Self {
        self.descriptors.extend(descriptors);
        self
    }

    pub fn skills(mut self, descriptors: impl IntoIterator<Item = CapabilityDescriptor>) -> Self {
        self.descriptors.extend(descriptors);
        self
    }

    pub fn mcp(mut self, descriptors: impl IntoIterator<Item = CapabilityDescriptor>) -> Self {
        self.descriptors.extend(descriptors);
        self
    }

    pub fn build(self) -> Result<CapabilityCatalog, CapabilityCatalogError> {
        CapabilityCatalog::new(self.descriptors)
    }
}

#[derive(Debug, Clone, Default)]
pub struct CapabilityCatalog {
    by_id: BTreeMap<String, Arc<CapabilityDescriptor>>,
    alias_index: BTreeMap<String, Vec<String>>,
}

impl CapabilityCatalog {
    pub fn new(descriptors: Vec<CapabilityDescriptor>) -> Result<Self, CapabilityCatalogError> {
        let mut by_id = BTreeMap::new();
        for descriptor in descriptors {
            if !descriptor.has_valid_version_hash() {
                return Err(CapabilityCatalogError::InvalidVersionHash(descriptor.id));
            }
            let id = descriptor.id.clone();
            if by_id.insert(id.clone(), Arc::new(descriptor)).is_some() {
                return Err(CapabilityCatalogError::DuplicateId(id));
            }
        }

        let mut alias_index = BTreeMap::<String, Vec<String>>::new();
        for (id, descriptor) in &by_id {
            for alias in &descriptor.aliases {
                let alias = normalize_alias(alias);
                if !alias.is_empty() {
                    alias_index.entry(alias).or_default().push(id.clone());
                }
            }
        }
        for ids in alias_index.values_mut() {
            ids.sort();
            ids.dedup();
        }

        Ok(Self { by_id, alias_index })
    }

    pub fn build(
        flows: impl IntoIterator<Item = CapabilityDescriptor>,
        skills: impl IntoIterator<Item = CapabilityDescriptor>,
        mcp: impl IntoIterator<Item = CapabilityDescriptor>,
    ) -> Result<Self, CapabilityCatalogError> {
        CapabilityCatalogBuilder::new()
            .flows(flows)
            .skills(skills)
            .mcp(mcp)
            .build()
    }

    pub fn get(&self, id: &str) -> Option<Arc<CapabilityDescriptor>> {
        self.by_id.get(id).cloned()
    }

    pub fn all(&self) -> Vec<Arc<CapabilityDescriptor>> {
        self.by_id.values().cloned().collect()
    }

    pub fn exact_alias(&self, utterance: &str) -> Vec<Arc<CapabilityDescriptor>> {
        self.alias_index
            .get(&normalize_alias(utterance))
            .into_iter()
            .flatten()
            .filter_map(|id| self.get(id))
            .collect()
    }

    pub fn visible_for(&self, profile: &AgentProfile) -> Vec<Arc<CapabilityDescriptor>> {
        self.by_id
            .values()
            .filter(|descriptor| descriptor.enabled)
            .filter(|descriptor| descriptor.risk <= profile.max_auto_risk)
            .filter(|descriptor| {
                profile.visible_capabilities.is_empty()
                    || profile.visible_capabilities.contains(&descriptor.id)
            })
            .cloned()
            .collect()
    }
}

fn normalize_alias(alias: &str) -> String {
    alias.trim().to_lowercase()
}
