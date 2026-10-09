use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

pub const KINDS: [&str; 6] = ["identity", "task", "capability", "mcp", "session", "device"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub schema_version: u32,
    pub revision: String,
    pub entity_id: String,
    pub kind: String,
    pub name: String,
    pub data: Value,
    pub deleted: bool,
    pub parents: Vec<String>,
    pub device_id: String,
    pub timestamp_ms: u64,
}

impl Event {
    pub fn validate(&self) -> Result<()> {
        for id in [&self.revision, &self.entity_id, &self.device_id] {
            if Uuid::parse_str(id).ok().map(|u| u.to_string()).as_ref() != Some(id) {
                return Err(Error::new(
                    "invalid_event",
                    "Identifiers must be canonical lowercase UUIDs",
                ));
            }
        }
        if self.schema_version != 1 || !KINDS.contains(&self.kind.as_str()) {
            return Err(Error::new(
                "unsupported_schema",
                "Unsupported entity schema or kind",
            ));
        }
        if self.name.trim().is_empty() || self.name.len() > 1024 || !self.data.is_object() {
            return Err(Error::new(
                "invalid_entity",
                "Name is required (max 1024 bytes); data must be an object",
            ));
        }
        let mut unique = BTreeSet::new();
        for parent in &self.parents {
            if Uuid::parse_str(parent).ok().map(|u| u.to_string()).as_ref() != Some(parent) {
                return Err(Error::new(
                    "invalid_event",
                    "Parent must be a canonical lowercase UUID",
                ));
            }
            if parent == &self.revision || !unique.insert(parent) {
                return Err(Error::new("invalid_event", "Invalid or duplicate parent"));
            }
        }
        reject_credentials(&self.data)?;
        if self.kind == "task" {
            crate::task::Profile::parse(&self.data)?;
        }
        if self.kind == "capability" {
            crate::capability::Profile::parse(&self.data)?;
        }
        if self.kind == "identity" {
            crate::identity::Profile::parse(&self.data)?;
        }
        if serde_json::to_vec(self)?.len() > 1024 * 1024 {
            return Err(Error::new("input_too_large", "Event exceeds 1 MiB"));
        }
        if self.kind == "session" {
            if let Some(agent) = self.data.get("agent") {
                if !agent
                    .as_str()
                    .is_some_and(|id| !id.is_empty() && id.len() <= 128)
                {
                    return Err(Error::new(
                        "invalid_event",
                        "Session agent must be a nonempty identifier of at most 128 bytes",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn reject_credentials(value: &Value) -> Result<()> {
    match value {
        Value::Object(obj) => {
            for (key, value) in obj {
                let normalized = key.to_ascii_lowercase().replace('-', "_");
                if [
                    "password",
                    "api_key",
                    "apikey",
                    "access_token",
                    "refresh_token",
                    "private_key",
                    "secret",
                    "token",
                    "authorization",
                ]
                .contains(&normalized.as_str())
                {
                    return Err(Error::new(
                        "credential_not_allowed",
                        "Store credential references, not secret values, in synchronized objects",
                    ));
                }
                reject_credentials(value)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_credentials(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityView {
    pub id: String,
    pub kind: String,
    pub conflicted: bool,
    pub heads: Vec<Event>,
}
