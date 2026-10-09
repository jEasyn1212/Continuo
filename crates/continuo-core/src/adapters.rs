use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Serialize)]
pub struct Descriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub executable: &'static str,
    pub capabilities: Value,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    pub cwd: String,
    pub prompt: Option<String>,
    pub native_session_id: Option<String>,
    pub identity_instructions: Option<String>,
}
#[derive(Serialize)]
pub struct LaunchPlan {
    pub agent: String,
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub executed: bool,
    pub warnings: Vec<String>,
}

pub trait AgentAdapter: Send + Sync {
    fn descriptor(&self) -> Descriptor;
    fn prepare_launch(&self, request: &LaunchRequest) -> Result<LaunchPlan>;
    fn mcp_registration(&self, executable: &str, args: &[String]) -> Value;
}

/// Explicit registry: adding an agent never changes domain storage or API routing.
pub struct Registry {
    adapters: Vec<Box<dyn AgentAdapter>>,
}
impl Default for Registry {
    fn default() -> Self {
        Self {
            adapters: vec![Box::new(ClaudeCode), Box::new(Codex), Box::new(Hermes)],
        }
    }
}
impl Registry {
    pub fn new(adapters: Vec<Box<dyn AgentAdapter>>) -> Result<Self> {
        let mut ids = std::collections::BTreeSet::new();
        for adapter in &adapters {
            let id = adapter.descriptor().id;
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                || !ids.insert(id)
            {
                return Err(Error::new(
                    "invalid_adapter",
                    "Adapter IDs must be unique lowercase identifiers",
                ));
            }
        }
        Ok(Self { adapters })
    }
    pub fn list(&self) -> Vec<Descriptor> {
        self.adapters.iter().map(|a| a.descriptor()).collect()
    }
    pub fn get(&self, id: &str) -> Result<&dyn AgentAdapter> {
        self.adapters
            .iter()
            .find(|a| a.descriptor().id == id)
            .map(|a| a.as_ref())
            .ok_or_else(|| {
                Error::new(
                    "unsupported_agent",
                    format!(
                        "Supported agents: {}",
                        self.list()
                            .iter()
                            .map(|a| a.id)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })
    }
}

struct ClaudeCode;
struct Codex;
struct Hermes;
fn descriptor(
    id: &'static str,
    name: &'static str,
    executable: &'static str,
    identity: &str,
) -> Descriptor {
    Descriptor {
        id,
        name,
        executable,
        capabilities: json!({"launch_plan":true,"native_resume_plan":true,"mcp_stdio_registration":true,"identity_injection":identity,"config_apply":false,"process_execution":false,"internal_state_transfer":false,"runtime_verified":false}),
    }
}
fn base(descriptor: &Descriptor, r: &LaunchRequest) -> Result<LaunchPlan> {
    if !Path::new(&r.cwd).is_absolute() {
        return Err(Error::new(
            "invalid_path",
            "cwd must be an absolute path on the destination device",
        ));
    }
    if r.native_session_id
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.starts_with('-') || s.contains('\0'))
    {
        return Err(Error::new(
            "invalid_session",
            "Invalid native session identifier",
        ));
    }
    Ok(LaunchPlan {agent:descriptor.id.into(),executable:descriptor.executable.into(),args:vec![],cwd:r.cwd.clone(),env:BTreeMap::new(),executed:false,warnings:vec!["Plan only: verify the installed runtime version, login and permissions before executing.".into(),"Identity instructions are context, not account switching or security isolation.".into()]})
}
impl AgentAdapter for ClaudeCode {
    fn descriptor(&self) -> Descriptor {
        descriptor(
            "claude-code",
            "Claude Code",
            "claude",
            "append_system_prompt",
        )
    }
    fn prepare_launch(&self, r: &LaunchRequest) -> Result<LaunchPlan> {
        let mut p = base(&self.descriptor(), r)?;
        if let Some(id) = &r.native_session_id {
            p.args.push(format!("--resume={id}"));
        }
        if let Some(instructions) = &r.identity_instructions {
            p.args
                .extend(["--append-system-prompt".into(), instructions.clone()]);
        }
        if let Some(prompt) = &r.prompt {
            p.args.extend(["--".into(), prompt.clone()]);
        }
        Ok(p)
    }
    fn mcp_registration(&self, executable: &str, args: &[String]) -> Value {
        json!({"format":"json","document":{"mcpServers":{"continuo":{"type":"stdio","command":executable,"args":args}}}})
    }
}
impl AgentAdapter for Codex {
    fn descriptor(&self) -> Descriptor {
        descriptor("codex", "Codex", "codex", "developer_instructions_override")
    }
    fn prepare_launch(&self, r: &LaunchRequest) -> Result<LaunchPlan> {
        let mut p = base(&self.descriptor(), r)?;
        if r.native_session_id.is_some() {
            p.args.push("resume".into());
        }
        p.args.extend(["-C".into(), r.cwd.clone()]);
        if let Some(instructions) = &r.identity_instructions {
            p.args.extend([
                "-c".into(),
                format!(
                    "developer_instructions={}",
                    serde_json::to_string(instructions)?
                ),
            ]);
        }
        if r.native_session_id.is_some() || r.prompt.is_some() {
            p.args.push("--".into());
        }
        if let Some(id) = &r.native_session_id {
            p.args.push(id.clone());
        }
        if let Some(prompt) = &r.prompt {
            p.args.push(prompt.clone());
        }
        Ok(p)
    }
    fn mcp_registration(&self, executable: &str, args: &[String]) -> Value {
        json!({"format":"toml","document":{"mcp_servers":{"continuo":{"command":executable,"args":args}}}})
    }
}
impl AgentAdapter for Hermes {
    fn descriptor(&self) -> Descriptor {
        descriptor("hermes", "Hermes", "hermes", "initial_prompt")
    }
    fn prepare_launch(&self, r: &LaunchRequest) -> Result<LaunchPlan> {
        let mut p = base(&self.descriptor(), r)?;
        p.args.extend(["--in".into(), r.cwd.clone()]);
        if let Some(id) = &r.native_session_id {
            p.args.push(format!("--resume={id}"));
        }
        p.args.push("chat".into());
        let prompt = match (&r.identity_instructions, &r.prompt) {
            (Some(i), Some(q)) => Some(format!("Identity context:\n{i}\n\nTask:\n{q}")),
            (Some(i), None) => Some(format!("Identity context:\n{i}")),
            (None, q) => q.clone(),
        };
        if let Some(prompt) = prompt {
            p.args.push(format!("--query={prompt}"));
        }
        if r.identity_instructions.is_some() {
            p.warnings.push(
                "Hermes identity context is sent as an initial user prompt in this prototype."
                    .into(),
            );
        }
        Ok(p)
    }
    fn mcp_registration(&self, executable: &str, args: &[String]) -> Value {
        json!({"format":"yaml","document":{"mcp_servers":{"continuo":{"command":executable,"args":args,"enabled":true}}}})
    }
}
