//! Portable MCP definitions and device-local mappings. Connection checks never call tools.
mod probe;
use crate::{adapters::AgentAdapter, model::Event, store::Store, Error, Result};
use fs2::FileExt;
use rusqlite::{Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    path::Path,
};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub description: String,
    pub transport: String,
    pub server_key: String,
    pub command_hint: String,
    pub endpoint: Option<String>,
    pub required_env: Vec<String>,
    pub credential_refs: Vec<String>,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            description: String::new(),
            transport: "stdio".into(),
            server_key: String::new(),
            command_hint: String::new(),
            endpoint: None,
            required_env: vec![],
            credential_refs: vec![],
        }
    }
}
fn env_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit()))
}
impl Profile {
    pub fn parse(data: &Value) -> Result<Self> {
        if [
            "command",
            "executable",
            "args",
            "cwd",
            "env",
            "env_refs",
            "mapping",
            "connection",
            "probe_result",
        ]
        .iter()
        .any(|key| data.get(key).is_some())
        {
            return Err(Error::new(
                "invalid_mcp_profile",
                "Device commands, environment and connection state belong in local mappings, not portable definitions",
            ));
        }
        let p: Self = serde_json::from_value(data.clone())
            .map_err(|_| Error::new("invalid_mcp_profile", "Invalid MCP field types"))?;
        let mut names = BTreeSet::new();
        let mut refs = BTreeSet::new();
        if p.description.len() > 8192
            || !["stdio", "http"].contains(&p.transport.as_str())
            || p.server_key.len() > 128
            || !p
                .server_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_ .".contains(&b))
            || p.server_key.contains(' ')
            || p.command_hint.len() > 256
            || p.command_hint.contains(['/', '\\'])
            || p.command_hint.chars().any(char::is_control)
            || p.endpoint
                .as_ref()
                .is_some_and(|s| !crate::task::reference(s) || !s.starts_with("https://"))
            || p.required_env.len() > 64
            || p.required_env
                .iter()
                .any(|s| !env_name(s) || !names.insert(s))
            || p.credential_refs.len() > 64
            || p.credential_refs.iter().any(|s| {
                s.len() > 512
                    || !s.split_once(':').is_some_and(|(a, b)| {
                        ["env", "keychain", "credential"].contains(&a)
                            && !b.is_empty()
                            && b.bytes()
                                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
                    })
                    || !refs.insert(s)
            })
        {
            return Err(Error::new("invalid_mcp_profile","Invalid transport, portable endpoint, server key, environment names or credential references"));
        }
        Ok(p)
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Mapping {
    pub revision: String,
    pub cleared: bool,
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env_refs: BTreeMap<String, String>,
}
impl Mapping {
    fn validate(&self) -> Result<()> {
        if serde_json::to_vec(self)?.len() > 65536
            || !Path::new(&self.executable).is_absolute()
            || self.executable.len() > 8192
            || self.executable.contains('\0')
            || self.args.len() > 128
            || self.args.iter().any(|s| s.len() > 8192 || s.contains('\0'))
            || self
                .cwd
                .as_ref()
                .is_some_and(|p| !Path::new(p).is_absolute() || p.len() > 8192 || p.contains('\0'))
            || self.env_refs.len() > 64
            || self
                .env_refs
                .iter()
                .any(|(k, v)| !env_name(k) || !v.strip_prefix("env:").is_some_and(env_name))
        {
            return Err(Error::new("invalid_mcp_mapping","Use absolute device-local paths, literal argv and env:NAME references; do not store sensitive values"));
        }
        Ok(())
    }
}
pub(crate) fn live(store: &Store, id: &str) -> Result<Event> {
    let v = store.get(id)?;
    if v.kind != "mcp" || v.conflicted || v.heads[0].deleted {
        return Err(Error::new(
            "mcp_unavailable",
            "MCP definition is deleted, conflicted or has the wrong kind",
        ));
    }
    Ok(v.heads[0].clone())
}
fn key(id: &str) -> String {
    format!("mcp_mapping:{id}")
}
fn probe_key(id: &str) -> String {
    format!("mcp_probe:{id}")
}
pub(crate) fn mapping(store: &Store, id: &str) -> Result<Option<Mapping>> {
    store
        .metadata(&key(id))?
        .map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}
fn state(store: &Store, id: &str) -> Result<Value> {
    Ok(store
        .metadata(&probe_key(id))?
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or(json!({"state":"not_checked"})))
}
fn issues(profile: &Profile, map: Option<&Mapping>) -> Vec<Value> {
    let mut out = vec![];
    if profile.server_key.is_empty() {
        out.push(json!({"code":"server_key_missing"}));
    }
    if profile.transport != "stdio" {
        out.push(json!({"code":"transport_unsupported"}));
        return out;
    }
    match map {
        None => out.push(json!({"code":"mapping_missing"})),
        Some(m) => {
            if m.cleared {
                out.push(json!({"code":"mapping_missing"}));
                return out;
            }
            if !Path::new(&m.executable).is_file() {
                out.push(json!({"code":"executable_missing"}));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if std::fs::metadata(&m.executable)
                    .is_ok_and(|m| m.permissions().mode() & 0o111 == 0)
                {
                    out.push(json!({"code":"executable_not_executable"}));
                }
            }
            if m.cwd.as_ref().is_some_and(|p| !Path::new(p).is_dir()) {
                out.push(json!({"code":"cwd_missing"}));
            }
            for name in &profile.required_env {
                if !m.env_refs.contains_key(name) {
                    out.push(json!({"code":"env_reference_missing","name":name}));
                }
            }
            if !m.env_refs.is_empty() || !profile.credential_refs.is_empty() {
                out.push(json!({"code":"credential_resolution_pending"}));
            }
        }
    }
    out
}
pub fn inspect(store: &Store, id: &str, adapter: &dyn AgentAdapter, probes: bool) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred)?;
    let e = live(store, id)?;
    let p = Profile::parse(&e.data)?;
    let m = mapping(store, id)?;
    let checks = issues(&p, m.as_ref());
    let mut connection = state(store, id)?;
    if connection["state"] != "not_checked"
        && (connection["definition_revision"] != e.revision
            || connection["mapping_revision"]
                != m.as_ref().map(|m| json!(m.revision)).unwrap_or(Value::Null))
    {
        connection["state"] = json!("stale");
    }
    if connection["state"] == "running" {
        let path = store.data_dir.join(format!("mcp-probe-{id}.lock"));
        if let Ok(file) = OpenOptions::new().read(true).write(true).open(path) {
            if file.try_lock_exclusive().is_ok() {
                connection["state"] = json!("interrupted");
            }
        }
    }
    let adaptation = if p.transport == "stdio" {
        match adapter.prepare_mcp(
            &p.server_key,
            m.as_ref()
                .map(|m| m.executable.as_str())
                .unwrap_or("/device-local-placeholder"),
            m.as_ref().map(|m| m.args.as_slice()).unwrap_or(&[]),
            m.as_ref().and_then(|m| m.cwd.as_deref()),
        ) {
            Ok(_) => {
                json!({"supported":true,"target_agent":adapter.descriptor().id,"runtime_verified":false})
            }
            Err(e) => {
                json!({"supported":false,"target_agent":adapter.descriptor().id,"error":e,"runtime_verified":false})
            }
        }
    } else {
        json!({"supported":false,"target_agent":adapter.descriptor().id,"runtime_verified":false})
    };
    let out = json!({"definition":e,"profile":p,"mapping":m,"mapping_scope":"device","device_id":store.device_id()?,"issues":checks,"ready":checks.is_empty(),"adaptation":adaptation,"connection":connection,"authorization":{"probes_enabled":probes,"per_request_confirmation_required":true,"tools_invocation_allowed":false,"persistent_permissions_granted":false}});
    tx.commit()?;
    Ok(out)
}
pub fn map(
    store: &Store,
    id: &str,
    expected: &str,
    expected_map: &str,
    data: Value,
) -> Result<Value> {
    let mut new: Mapping = serde_json::from_value(data)
        .map_err(|_| Error::new("invalid_mcp_mapping", "Invalid mapping field types"))?;
    new.cleared = false;
    new.validate()?;
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    let e = live(store, id)?;
    if e.revision != expected
        || mapping(store, id)?
            .as_ref()
            .map(|m| m.revision.as_str())
            .unwrap_or("none")
            != expected_map
    {
        return Err(Error::new(
            "mcp_mapping_conflict",
            "Definition or device mapping changed; read it again",
        ));
    }
    new.revision = Uuid::new_v4().to_string();
    store.set_metadata(&key(id), &serde_json::to_string(&new)?)?;
    tx.commit()?;
    Ok(json!({"mapping":new,"scope":"device","device_id":store.device_id()?}))
}
pub fn clear(store: &Store, id: &str, expected_map: &str) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    if mapping(store, id)?
        .as_ref()
        .map(|m| m.revision.as_str())
        .unwrap_or("none")
        != expected_map
    {
        return Err(Error::new("mcp_mapping_conflict", "Device mapping changed"));
    }
    store.set_metadata(
        &key(id),
        &serde_json::to_string(&Mapping {
            revision: Uuid::new_v4().to_string(),
            cleared: true,
            ..Mapping::default()
        })?,
    )?;
    tx.commit()?;
    Ok(json!({"cleared":true,"scope":"device"}))
}
pub fn prepare(
    store: &Store,
    id: &str,
    expected: Option<&str>,
    adapter: &dyn AgentAdapter,
) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred)?;
    let e = live(store, id)?;
    if expected.is_some_and(|r| r != e.revision) {
        return Err(Error::new("revision_conflict", "MCP definition changed"));
    }
    let p = Profile::parse(&e.data)?;
    let m = mapping(store, id)?;
    let checks = issues(&p, m.as_ref());
    if !checks.is_empty() {
        return Err(Error::new(
            "mcp_not_ready",
            "Repair the definition and device mapping before registration",
        )
        .details(json!({"issues":checks})));
    }
    let m = m.unwrap();
    let registration =
        adapter.prepare_mcp(&p.server_key, &m.executable, &m.args, m.cwd.as_deref())?;
    let out = json!({"id":id,"definition_revision":e.revision,"mapping_revision":m.revision,"scope":"device","target_agent":adapter.descriptor().id,"registration":registration,"config_written":false,"executed":false,"connection_verified":false,"permissions_granted":false});
    tx.commit()?;
    Ok(out)
}
pub fn cancel(store: &Store, id: &str, probe_id: &str) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    let mut current = state(store, id)?;
    if current["probe_id"] != probe_id || current["state"] != "running" {
        return Err(Error::new(
            "mcp_probe_conflict",
            "The selected check is no longer running",
        ));
    }
    current["cancel_requested"] = json!(true);
    store.set_metadata(&probe_key(id), &current.to_string())?;
    tx.commit()?;
    Ok(json!({"cancel_requested":true,"probe_id":probe_id}))
}
pub fn check(
    store: &Store,
    id: &str,
    expected: &str,
    expected_map: &str,
    probe_id: &str,
    confirmed: bool,
    timeout: u64,
) -> Result<Value> {
    #[cfg(not(unix))]
    {
        return Err(Error::new(
            "mcp_probe_platform_unsupported",
            "Bounded process cleanup is currently implemented only on macOS/Linux",
        ));
    }
    if !confirmed {
        return Err(Error::new(
            "mcp_confirmation_required",
            "Explicitly confirm the exact device-local command before running a one-off check",
        ));
    }
    if Uuid::parse_str(probe_id)
        .ok()
        .map(|u| u.to_string())
        .as_deref()
        != Some(probe_id)
        || !(100..=5000).contains(&timeout)
    {
        return Err(Error::new(
            "invalid_params",
            "Probe ID must be a canonical UUID; timeout must be 100..5000 ms",
        ));
    }
    let e = live(store, id)?;
    let p = Profile::parse(&e.data)?;
    let m = mapping(store, id)?;
    if e.revision != expected || m.as_ref().map(|m| m.revision.as_str()) != Some(expected_map) {
        return Err(Error::new(
            "mcp_mapping_conflict",
            "Definition or mapping changed before confirmation",
        ));
    }
    let checks = issues(&p, m.as_ref());
    if !checks.is_empty() {
        return Err(
            Error::new("mcp_not_ready", "Device environment is not ready")
                .details(json!({"issues":checks})),
        );
    }
    let m = m.unwrap();
    let lease = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(store.data_dir.join(format!("mcp-probe-{id}.lock")))?;
    lease
        .try_lock_exclusive()
        .map_err(|_| Error::new("mcp_busy", "A check of this connection is already running"))?;
    let mut status = json!({"probe_id":probe_id,"definition_revision":expected,"mapping_revision":expected_map,"state":"running","cancel_requested":false,"tools_called":false,"permissions_granted":false});
    {
        let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
        if live(store, id)?.revision != expected
            || mapping(store, id)?.as_ref().map(|m| m.revision.as_str()) != Some(expected_map)
        {
            return Err(Error::new(
                "mcp_mapping_conflict",
                "Definition or mapping changed",
            ));
        }
        let probe_marker = format!("mcp_probe_id:{probe_id}");
        if store.metadata(&probe_marker)?.is_some() {
            return Err(Error::new(
                "mcp_probe_conflict",
                "A connection check ID cannot be reused",
            ));
        }
        store.set_metadata(&probe_marker, "used")?;
        store.set_metadata(&probe_key(id), &status.to_string())?;
        tx.commit()?;
    }
    let result = probe::run(&m, timeout, || {
        Ok(state(store, id)?["cancel_requested"] == true)
    });
    match result {
        Ok(info) => {
            status["state"] = json!("succeeded");
            status["result"] = info;
        }
        Err(error) => {
            status["state"] = json!(match error.code.as_str() {
                "mcp_probe_timeout" => "timeout",
                "mcp_probe_cancelled" => "cancelled",
                _ => "failed",
            });
            status["error"] = serde_json::to_value(error)?;
        }
    }
    // Cancellation and a concurrently edited definition are retained; success is only this snapshot.
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    status["cancel_requested"] = state(store, id)?["cancel_requested"].clone();
    store.set_metadata(&probe_key(id), &status.to_string())?;
    tx.commit()?;
    drop(lease);
    Ok(status)
}
