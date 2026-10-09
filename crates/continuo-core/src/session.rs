//! Portable session references and device-local prerequisites, never agent history imports.
use crate::{adapters::Registry, identity, model::Event, store::Store, task, Error, Result};
use rusqlite::{Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
pub const STATES: [&str; 5] = ["registered", "active", "paused", "closed", "cancelled"];
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub agent: String,
    pub native_session_id: Option<String>,
    pub task_id: Option<String>,
    pub identity_id: Option<String>,
    pub origin_device_id: Option<String>,
    pub environment_ref: Option<String>,
    pub status: String,
    pub source: String,
    pub source_ref: Option<String>,
    pub summary: String,
    pub state_log: Vec<StateEntry>,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            agent: String::new(),
            native_session_id: None,
            task_id: None,
            identity_id: None,
            origin_device_id: None,
            environment_ref: None,
            status: "registered".into(),
            source: "manual".into(),
            source_ref: None,
            summary: String::new(),
            state_log: vec![],
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateEntry {
    pub id: String,
    pub from: String,
    pub to: String,
    pub reason: String,
    pub device_id: String,
    pub recorded_at_ms: u64,
}
fn uuid(s: &str) -> bool {
    Uuid::parse_str(s).ok().is_some_and(|u| u.to_string() == s)
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
impl Profile {
    pub fn parse(data: &Value) -> Result<Self> {
        if [
            "cwd",
            "executable",
            "env",
            "account_ref",
            "account",
            "account_id",
            "env_refs",
            "runtime_version",
            "state_present_confirmed",
            "runtime_state",
            "messages",
            "conversation",
            "transcript",
            "mapping",
        ]
        .iter()
        .any(|k| data.get(k).is_some())
        {
            return Err(Error::new("invalid_session_profile","Keep local paths, accounts and raw runtime history outside portable session references"));
        }
        let p: Self = serde_json::from_value(data.clone())
            .map_err(|_| Error::new("invalid_session_profile", "Invalid session field types"))?;
        if (!p.agent.is_empty() && !identifier(&p.agent))
            || p.native_session_id.as_ref().is_some_and(|s| !identifier(s))
            || [&p.task_id, &p.identity_id, &p.origin_device_id]
                .iter()
                .any(|s| s.as_ref().is_some_and(|s| !uuid(s)))
            || [&p.environment_ref, &p.source_ref]
                .iter()
                .any(|s| s.as_ref().is_some_and(|s| !task::reference(s)))
            || !STATES.contains(&p.status.as_str())
            || !["manual", "referenced"].contains(&p.source.as_str())
            || p.summary.len() > 32768
            || p.state_log.len() > 128
            || p.state_log.iter().any(|e| {
                !uuid(&e.id)
                    || !uuid(&e.device_id)
                    || !STATES.contains(&e.from.as_str())
                    || !next_states(&e.from).contains(&e.to.as_str())
                    || e.reason.trim().is_empty()
                    || e.reason.len() > 8192
            })
        {
            return Err(Error::new(
                "invalid_session_profile",
                "Invalid session identifiers, portable references, source or recorded status",
            ));
        }
        Ok(p)
    }
    fn binding(&self) -> Result<String> {
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&json!([
                self.agent,
                self.native_session_id,
                self.origin_device_id,
                self.environment_ref
            ]))?)
        ))
    }
}
pub fn next_states(s: &str) -> &'static [&'static str] {
    match s {
        "registered" => &["active", "paused", "closed", "cancelled"],
        "active" => &["paused", "closed", "cancelled"],
        "paused" => &["active", "closed", "cancelled"],
        "closed" | "cancelled" => &["paused"],
        _ => &[],
    }
}
fn live(store: &Store, id: &str) -> Result<Event> {
    let v = store.get(id)?;
    if v.kind != "session" || v.conflicted || v.heads[0].deleted {
        return Err(Error::new(
            "session_unavailable",
            "Session is deleted, conflicted or has the wrong kind",
        ));
    }
    Ok(v.heads[0].clone())
}
fn relation(store: &Store, id: &str, kind: &str) -> Result<Event> {
    let v = store.get(id).map_err(|e| {
        if e.code == "not_found" {
            Error::new("session_relation_unavailable", "Linked record is missing")
        } else {
            e
        }
    })?;
    if v.kind != kind || v.conflicted || v.heads[0].deleted {
        return Err(Error::new(
            "session_relation_unavailable",
            "Choose a live linked record without conflicts",
        ));
    }
    Ok(v.heads[0].clone())
}
pub(crate) fn validate_local(store: &Store, event: &Event) -> Result<()> {
    if event.kind != "session" || event.deleted {
        return Ok(());
    }
    let p = Profile::parse(&event.data)?;
    if let Some(id) = &p.identity_id {
        relation(store, id, "identity")?;
    }
    if let Some(id) = &p.task_id {
        let e = relation(store, id, "task")?;
        let t = task::Profile::parse(&e.data)?;
        if p.identity_id.is_some() && t.identity_id.is_some() && p.identity_id != t.identity_id {
            return Err(Error::new(
                "session_identity_mismatch",
                "Session identity differs from the linked task assignment",
            ));
        }
    }
    if event.parents.len() == 1 {
        let previous = store.get(&event.entity_id)?;
        if previous.heads[0].deleted {
            return Ok(());
        }
        let old = Profile::parse(&previous.heads[0].data)?;
        if old.status != p.status {
            if !next_states(&old.status).contains(&p.status.as_str()) {
                return Err(Error::new(
                    "invalid_session_transition",
                    "Choose an allowed recorded status transition",
                ));
            }
            if !p.state_log.iter().any(|e| {
                !old.state_log.iter().any(|o| o.id == e.id)
                    && e.from == old.status
                    && e.to == p.status
            }) {
                return Err(Error::new(
                    "session_transition_requires_reason",
                    "Use session.transition to append a recorded reason",
                ));
            }
        }
    }
    Ok(())
}
pub fn register(store: &Store, name: &str, mut data: Value) -> Result<Value> {
    Profile::parse(&data)?;
    if data.get("origin_device_id").is_none_or(Value::is_null) {
        data["origin_device_id"] = json!(store.device_id()?);
    }
    Ok(serde_json::to_value(store.create("session", name, data)?)?)
}
pub fn transition(
    store: &Store,
    id: &str,
    expected: &str,
    status: &str,
    reason: &str,
) -> Result<Value> {
    let e = live(store, id)?;
    if e.revision != expected {
        return Err(Error::new(
            "revision_conflict",
            "Session changed before status recording",
        ));
    }
    let p = Profile::parse(&e.data)?;
    if !next_states(&p.status).contains(&status) {
        return Err(Error::new(
            "invalid_session_transition",
            "Choose an allowed status transition",
        ));
    }
    if reason.trim().is_empty() || reason.len() > 8192 {
        return Err(Error::new(
            "invalid_session_profile",
            "A bounded nonempty reason is required",
        ));
    }
    let mut d = e.data;
    d["status"] = json!(status);
    let mut log = p.state_log;
    log.push(StateEntry {
        id: Uuid::new_v4().to_string(),
        from: p.status,
        to: status.into(),
        reason: reason.into(),
        device_id: store.device_id()?,
        recorded_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    });
    d["state_log"] = json!(log);
    Ok(serde_json::to_value(store.update(
        id,
        expected,
        None,
        Some(d),
        false,
    )?)?)
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Mapping {
    pub revision: String,
    pub cleared: bool,
    pub binding_digest: String,
    pub executable: String,
    pub cwd: String,
    pub account_ref: Option<String>,
    pub runtime_version: String,
    pub state_present_confirmed: bool,
}
impl Mapping {
    fn validate(&self) -> Result<()> {
        if serde_json::to_vec(self)?.len() > 65536
            || !Path::new(&self.executable).is_absolute()
            || !Path::new(&self.cwd).is_absolute()
            || [&self.executable, &self.cwd]
                .iter()
                .any(|s| s.len() > 8192 || s.contains('\0'))
            || self.runtime_version.len() > 256
            || self.account_ref.as_ref().is_some_and(|s| {
                !s.split_once(':').is_some_and(|(a, b)| {
                    ["credential", "keychain", "env"].contains(&a) && identifier(b)
                })
            })
        {
            return Err(Error::new("invalid_session_mapping","Use absolute device paths, a credential reference and explicit local-state confirmation"));
        }
        Ok(())
    }
}
fn map_key(id: &str) -> String {
    format!("session_mapping:{id}")
}
fn mapping(store: &Store, id: &str) -> Result<Option<Mapping>> {
    store
        .metadata(&map_key(id))?
        .map(|s| Ok(serde_json::from_str(&s)?))
        .transpose()
}
pub fn map(
    store: &Store,
    id: &str,
    expected: &str,
    expected_map: &str,
    value: Value,
) -> Result<Value> {
    let mut m: Mapping = serde_json::from_value(value)
        .map_err(|_| Error::new("invalid_session_mapping", "Invalid local mapping fields"))?;
    m.validate()?;
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    let e = live(store, id)?;
    let old = mapping(store, id)?;
    if e.revision != expected
        || old.as_ref().map(|m| m.revision.as_str()).unwrap_or("none") != expected_map
    {
        return Err(Error::new(
            "session_mapping_conflict",
            "Session or local environment changed",
        ));
    }
    let p = Profile::parse(&e.data)?;
    m.binding_digest = p.binding()?;
    m.revision = Uuid::new_v4().to_string();
    m.cleared = false;
    store.set_metadata(&map_key(id), &serde_json::to_string(&m)?)?;
    tx.commit()?;
    Ok(json!({"mapping":m,"scope":"device"}))
}
pub fn clear(store: &Store, id: &str, expected: &str) -> Result<Value> {
    if !uuid(id) {
        return Err(Error::new("invalid_params", "Invalid session ID"));
    }
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    let old = mapping(store, id)?;
    if old.as_ref().map(|m| m.revision.as_str()).unwrap_or("none") != expected {
        return Err(Error::new(
            "session_mapping_conflict",
            "Local environment changed",
        ));
    }
    let m = Mapping {
        revision: Uuid::new_v4().to_string(),
        cleared: true,
        ..Mapping::default()
    };
    store.set_metadata(&map_key(id), &serde_json::to_string(&m)?)?;
    tx.commit()?;
    Ok(json!({"cleared":true,"mapping_revision":m.revision,"scope":"device"}))
}
fn inspect_inner(store: &Store, id: &str, registry: &Registry) -> Result<Value> {
    let e = live(store, id)?;
    let p = Profile::parse(&e.data)?;
    let m = mapping(store, id)?;
    let mut links = vec![];
    let mut resume = vec![];
    let mut continuation = vec![];
    let mut task_context = Value::Null;
    let mut identity_context = Value::Null;
    if p.agent.is_empty() {
        links.push(json!({"code":"session_agent_missing"}));
    }
    if ["closed", "cancelled"].contains(&p.status.as_str()) {
        links.push(json!({"code":"session_closed"}));
    }
    if let Some(id) = &p.task_id {
        match task::inspect_inner(store, id) {
            Ok(c) => {
                if c["handoff_ready"] != true {
                    continuation.push(json!({"code":"session_task_not_ready"}));
                }
                task_context = c;
            }
            Err(err)
                if ["not_found", "invalid_task", "invalid_task_profile"]
                    .contains(&err.code.as_str()) =>
            {
                links.push(json!({"code":"session_task_unavailable"}))
            }
            Err(err) => return Err(err),
        }
    } else {
        continuation.push(json!({"code":"session_task_missing"}));
    }
    let task_identity = task_context["profile"]["identity_id"].as_str();
    let effective = p.identity_id.as_deref().or(task_identity);
    if p.identity_id.is_some()
        && task_identity.is_some()
        && p.identity_id.as_deref() != task_identity
    {
        links.push(json!({"code":"session_identity_mismatch"}));
    }
    if let Some(id) = effective {
        match identity::inspect_inner(store, id) {
            Ok(c) => {
                if c["ready"] != true {
                    links.push(json!({"code":"session_identity_unavailable"}));
                }
                identity_context = c;
            }
            Err(err)
                if ["not_found", "invalid_identity", "invalid_identity_profile"]
                    .contains(&err.code.as_str()) =>
            {
                links.push(json!({"code":"session_identity_unavailable"}))
            }
            Err(err) => return Err(err),
        }
    }
    if p.native_session_id.is_none() {
        resume.push(json!({"code":"native_session_missing"}));
    }
    if !registry
        .get(&p.agent)
        .is_ok_and(|a| a.descriptor().capabilities["native_resume_plan"] == true)
    {
        resume.push(json!({"code":"session_adapter_unsupported"}));
    }
    if let Some(m) = m.as_ref().filter(|m| !m.cleared) {
        if m.binding_digest != p.binding()? {
            resume.push(json!({"code":"session_mapping_stale"}));
        }
        if !Path::new(&m.executable).is_file() {
            resume.push(json!({"code":"session_executable_missing"}));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&m.executable).is_ok_and(|v| v.permissions().mode() & 0o111 == 0) {
                resume.push(json!({"code":"session_executable_not_executable"}));
            }
        }
        if !Path::new(&m.cwd).is_dir() {
            resume.push(json!({"code":"session_cwd_missing"}));
        }
        if !m.state_present_confirmed {
            resume.push(json!({"code":"native_state_unconfirmed"}));
        }
    } else {
        resume.push(json!({"code":"session_mapping_missing"}));
    }
    resume.extend(links.iter().cloned());
    continuation.extend(links.iter().cloned());
    Ok(
        json!({"session":e,"profile":p,"mapping":m,"mapping_scope":"device","device_id":store.device_id()?,"allowed_transitions":next_states(&p.status),"relationships":links,"resume":{"ready":resume.is_empty(),"issues":resume,"native_state_verification":"user_confirmed_not_independently_verified","account_authentication_verified":false},"continuation":{"ready":continuation.is_empty(),"issues":continuation},"task_context":task_context,"identity_context":identity_context,"status_verification":"user_recorded_not_process_observation","history_read":false,"internal_state_transferred":false}),
    )
}
pub fn inspect(store: &Store, id: &str, registry: &Registry) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred)?;
    let r = inspect_inner(store, id, registry)?;
    tx.commit()?;
    Ok(r)
}
pub fn resume_plan(
    store: &Store,
    id: &str,
    expected: &str,
    expected_map: &str,
    registry: &Registry,
) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred)?;
    let i = inspect_inner(store, id, registry)?;
    if i["session"]["revision"] != expected || i["mapping"]["revision"] != expected_map {
        return Err(Error::new(
            "session_mapping_conflict",
            "Session or local environment changed before planning",
        ));
    }
    if i["resume"]["ready"] != true {
        return Err(
            Error::new("session_not_ready", "Repair local resume prerequisites").details(i),
        );
    }
    let p: Profile = serde_json::from_value(i["profile"].clone())?;
    let m: Mapping = serde_json::from_value(i["mapping"].clone())?;
    let mut plan = serde_json::to_value(
        registry
            .get(&p.agent)?
            .prepare_resume(&m.cwd, p.native_session_id.as_deref().unwrap())?,
    )?;
    plan["executable"] = json!(m.executable);
    plan["mode"] = json!("native_resume");
    plan["session_id"] = json!(id);
    plan["session_revision"] = json!(expected);
    plan["mapping_revision"] = json!(expected_map);
    plan["native_state_verification"] = json!("user_confirmed_not_independently_verified");
    plan["account_authentication_verified"] = json!(false);
    plan["identity_instructions_reapplied"] = json!(false);
    plan["internal_state_transferred"] = json!(false);
    tx.commit()?;
    Ok(plan)
}
pub fn packet(
    store: &Store,
    id: &str,
    expected: &str,
    target: &str,
    expected_task: &str,
    registry: &Registry,
) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred)?;
    registry.get(target)?;
    let i = inspect_inner(store, id, registry)?;
    if i["session"]["revision"] != expected {
        return Err(Error::new(
            "revision_conflict",
            "Session changed before context preparation",
        ));
    }
    if i["continuation"]["ready"] != true {
        return Err(Error::new(
            "session_not_ready",
            "Repair task and identity links before handoff",
        )
        .details(i));
    }
    let p: Profile = serde_json::from_value(i["profile"].clone())?;
    let mut packet = task::handoff_inner(
        store,
        p.task_id.as_deref().unwrap(),
        target,
        Some(expected_task),
    )?;
    let context = json!({"session_id":id,"session_revision":expected,"source_agent":p.agent,"origin_device_id":p.origin_device_id,"environment_ref":p.environment_ref,"summary":p.summary,"source":p.source,"source_ref":p.source_ref,"identity_id":p.identity_id,"identity_revision":i["identity_context"]["identity"]["revision"],"identity_instructions":i["identity_context"]["profile"]["instructions"]});
    let instructions = format!(
        "Source session reference (context only, no native state migration):\n{}",
        serde_json::to_string_pretty(&context)?
    );
    packet["mode"] = json!("context_handoff");
    packet["session_context"] = context;
    packet["session_instructions"] = json!(instructions);
    packet["prompt"] = json!(format!(
        "{}\n\n{}",
        packet["prompt"].as_str().unwrap(),
        instructions
    ));
    packet["native_session_id_transferred"] = json!(false);
    packet["effective_identity_id"] = i["identity_context"]["identity"]["entity_id"].clone();
    tx.commit()?;
    Ok(packet)
}
