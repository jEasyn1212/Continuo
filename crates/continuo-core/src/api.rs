use crate::{
    adapters::{LaunchRequest, Registry},
    crypto, identity,
    model::KINDS,
    store::Store,
    sync::{self, SyncBackend},
    Error, Result,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone, Copy)]
pub struct Policy {
    pub writes: bool,
    pub sync: bool,
    pub admin: bool,
}
impl Policy {
    pub fn local_user() -> Self {
        Self {
            writes: true,
            sync: false,
            admin: true,
        }
    }
    pub fn read_only() -> Self {
        Self {
            writes: false,
            sync: false,
            admin: false,
        }
    }
}

#[derive(Serialize)]
pub struct Operation {
    pub method: &'static str,
    pub tool: String,
    pub description: &'static str,
    pub input_schema: Value,
    pub writes: bool,
    pub network: bool,
    pub admin: bool,
}
fn op(
    method: &'static str,
    description: &'static str,
    properties: Value,
    required: &[&str],
    writes: bool,
    network: bool,
    admin: bool,
) -> Operation {
    Operation {
        method,
        tool: format!("continuo_{}", method.replace('.', "_")),
        description,
        input_schema: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        writes,
        network,
        admin,
    }
}
pub fn operations() -> Vec<Operation> {
    operations_for(&Registry::default())
}
fn operations_for(registry: &Registry) -> Vec<Operation> {
    let string = json!({"type":"string","minLength":1});
    let data = json!({"type":"object"});
    let kind = json!({"type":"string","enum":KINDS});
    let agent =
        json!({"type":"string","enum":registry.list().iter().map(|a| a.id).collect::<Vec<_>>()});
    vec![
        op(
            "system.status",
            "Local device, entity counts, conflicts and synchronization status",
            json!({}),
            &[],
            false,
            false,
            false,
        ),
        op(
            "system.describe",
            "Describe the operations enabled for this interface and their input schemas",
            json!({}),
            &[],
            false,
            false,
            false,
        ),
        op(
            "entity.list",
            "List identities, tasks, capabilities, MCP definitions, sessions or devices",
            json!({"kind":kind,"include_deleted":{"type":"boolean"}}),
            &[],
            false,
            false,
            false,
        ),
        op(
            "entity.get",
            "Read all current heads of an entity, including tombstones",
            json!({"id":string}),
            &["id"],
            false,
            false,
            false,
        ),
        op(
            "entity.create",
            "Create a synchronized object; use credential references, never secrets",
            json!({"kind":kind,"name":string,"data":data}),
            &["kind", "name", "data"],
            true,
            false,
            false,
        ),
        op(
            "entity.update",
            "Replace data or name using optimistic concurrency; fetch revision first",
            json!({"id":string,"expected_revision":string,"name":string,"data":data}),
            &["id", "expected_revision"],
            true,
            false,
            false,
        ),
        op(
            "entity.delete",
            "Create a tombstone; history is retained and deletion synchronizes",
            json!({"id":string,"expected_revision":string}),
            &["id", "expected_revision"],
            true,
            false,
            false,
        ),
        op(
            "entity.resolve",
            "Resolve a conflict with a new version referencing every current head",
            json!({"id":string,"expected_heads":{"type":"array","items":{"type":"string"},"minItems":2,"uniqueItems":true},"name":string,"data":data,"deleted":{"type":"boolean"}}),
            &["id", "expected_heads", "name", "data"],
            true,
            false,
            false,
        ),
        op("identity.inspect", "Inspect a portable identity and check capability/MCP bindings without deploying them", json!({"id":string}), &["id"], false, false, false),
        op("identity.current", "Read device-local current identity, its selection revision and availability", json!({}), &[], false, false, false),
        op("identity.activate", "Select a live identity on this device with optimistic identity and selection revisions", json!({"id":string,"expected_revision":string,"expected_selection_revision":string}), &["id","expected_revision","expected_selection_revision"], true, false, false),
        op("identity.clear", "Clear device-local current identity using its selection revision", json!({"expected_selection_revision":string}), &["expected_selection_revision"], true, false, false),
        op(
            "agent.list",
            "Describe registered agent adapters and their limits",
            json!({}),
            &[],
            false,
            false,
            false,
        ),
        op(
            "agent.prepare",
            "Prepare argv and environment without starting an agent or editing native config",
            json!({"agent":agent,"cwd":string,"prompt":string,"native_session_id":string,"identity_id":string,"use_current_identity":{"type":"boolean"}}),
            &["agent", "cwd"],
            false,
            false,
            false,
        ),
        op(
            "agent.mcp_registration",
            "Produce registration document data for the selected agent; does not write config",
            json!({"agent":agent,"executable":string,"data_dir":string,"allow_writes":{"type":"boolean"},"allow_sync":{"type":"boolean"},"allow_admin":{"type":"boolean"}}),
            &["agent", "executable", "data_dir"],
            false,
            false,
            false,
        ),
        op(
            "session.handoff",
            "Build explicit task context for another agent; never migrate internal state",
            json!({"task_id":string,"target_agent":agent}),
            &["task_id", "target_agent"],
            false,
            false,
            false,
        ),
        op(
            "sync.key_generate",
            "Generate a non-overwriting 32-byte key file; keep it outside the Git repository",
            json!({"path":string}),
            &["path"],
            true,
            false,
            true,
        ),
        op(
            "sync.configure",
            "Configure user-owned GitHub storage and an existing local encryption key",
            json!({"remote":string,"key_file":string}),
            &["remote", "key_file"],
            true,
            false,
            true,
        ),
        op(
            "sync.run",
            "Exchange encrypted immutable events with the configured user-owned Git repository",
            json!({}),
            &[],
            true,
            true,
            false,
        ),
    ]
}

pub struct Service {
    pub store: Store,
    pub registry: Registry,
    pub policy: Policy,
}
impl Service {
    pub fn open(path: &Path, policy: Policy) -> Result<Self> {
        Ok(Self {
            store: Store::open(path)?,
            registry: Registry::default(),
            policy,
        })
    }
    pub fn available(&self) -> Vec<Operation> {
        operations_for(&self.registry)
            .into_iter()
            .filter(|o| {
                (!o.writes || self.policy.writes)
                    && (!o.network || self.policy.sync)
                    && (!o.admin || self.policy.admin)
            })
            .collect()
    }
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let operation = operations_for(&self.registry)
            .into_iter()
            .find(|o| o.method == method)
            .ok_or_else(|| Error::new("unknown_method", "Unknown API method"))?;
        if (operation.writes && !self.policy.writes)
            || (operation.network && !self.policy.sync)
            || (operation.admin && !self.policy.admin)
        {
            return Err(Error::new(
                "permission_denied",
                "Operation is not enabled for this interface",
            ));
        }
        validate(&operation.input_schema, &params)?;
        let text = |key: &str| {
            params
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| Error::new("invalid_params", format!("Missing string: {key}")))
        };
        match method {
            "system.status" => self.store.status(),
            "system.describe" => Ok(json!({"operations":self.available()})),
            "entity.list" => Ok(
                json!({"entities":self.store.list(params.get("kind").and_then(Value::as_str),params.get("include_deleted").and_then(Value::as_bool).unwrap_or(false))?}),
            ),
            "entity.get" => Ok(serde_json::to_value(self.store.get(text("id")?)?)?),
            "entity.create" => Ok(serde_json::to_value(self.store.create(
                text("kind")?,
                text("name")?,
                params["data"].clone(),
            )?)?),
            "entity.update" | "entity.delete" => Ok(serde_json::to_value(self.store.update(
                text("id")?,
                text("expected_revision")?,
                params.get("name").and_then(Value::as_str),
                params.get("data").cloned(),
                method == "entity.delete",
            )?)?),
            "entity.resolve" => {
                let heads: Vec<String> = serde_json::from_value(params["expected_heads"].clone())?;
                Ok(serde_json::to_value(
                    self.store.resolve(
                        text("id")?,
                        &heads,
                        text("name")?,
                        params["data"].clone(),
                        params
                            .get("deleted")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    )?,
                )?)
            }
            "identity.inspect" => identity::inspect(&self.store, text("id")?),
            "identity.current" => identity::current(&self.store),
            "identity.activate" => identity::select(
                &self.store,
                text("expected_selection_revision")?,
                Some((text("id")?, text("expected_revision")?)),
            ),
            "identity.clear" => {
                identity::select(&self.store, text("expected_selection_revision")?, None)
            }
            "agent.list" => Ok(json!({"adapters":self.registry.list()})),
            "agent.prepare" => {
                let context = identity::launch_context(
                    &self.store,
                    params.get("identity_id").and_then(Value::as_str),
                    params
                        .get("use_current_identity")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                )?;
                let instructions = context
                    .as_ref()
                    .and_then(|c| c["profile"]["instructions"].as_str())
                    .map(str::to_owned);
                let request = LaunchRequest {
                    cwd: text("cwd")?.into(),
                    prompt: params
                        .get("prompt")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    native_session_id: params
                        .get("native_session_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    identity_instructions: instructions,
                };
                let mut plan = serde_json::to_value(
                    self.registry
                        .get(text("agent")?)?
                        .prepare_launch(&request)?,
                )?;
                if context
                    .as_ref()
                    .is_some_and(|c| !c["bindings"].as_array().unwrap().is_empty())
                {
                    plan["warnings"].as_array_mut().unwrap().push(json!("Capability and MCP bindings are context records; native configuration is not installed by this plan."));
                }
                plan["identity_context"] = context.unwrap_or(Value::Null);
                Ok(plan)
            }

            "agent.mcp_registration" => {
                if params
                    .get("allow_sync")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    && !params
                        .get("allow_writes")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                {
                    return Err(Error::new(
                        "invalid_params",
                        "allow_sync requires allow_writes",
                    ));
                }
                if params
                    .get("allow_admin")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    && !params
                        .get("allow_writes")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                {
                    return Err(Error::new(
                        "invalid_params",
                        "allow_admin requires allow_writes",
                    ));
                }
                if !Path::new(text("executable")?).is_absolute()
                    || !Path::new(text("data_dir")?).is_absolute()
                {
                    return Err(Error::new(
                        "invalid_path",
                        "executable and data_dir must be absolute device-local paths",
                    ));
                }
                let mut args = vec!["--data-dir".into(), text("data_dir")?.into(), "mcp".into()];
                if params
                    .get("allow_writes")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    args.push("--allow-writes".into());
                }
                if params
                    .get("allow_sync")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    args.push("--allow-sync".into());
                }
                if params
                    .get("allow_admin")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    args.push("--allow-admin".into());
                }
                Ok(self
                    .registry
                    .get(text("agent")?)?
                    .mcp_registration(text("executable")?, &args))
            }
            "session.handoff" => {
                self.registry.get(text("target_agent")?)?;
                let view = self.store.get(text("task_id")?)?;
                if view.kind != "task" || view.conflicted || view.heads[0].deleted {
                    return Err(Error::new(
                        "invalid_task",
                        "Choose a live task without unresolved conflicts",
                    ));
                }
                let task = &view.heads[0];
                Ok(
                    json!({"task_id":task.entity_id,"task_revision":task.revision,"target_agent":text("target_agent")?,"internal_state_transferred":false,"context":{"goal":task.name,"record":task.data},"prompt":format!("Continue this task using the recorded goals, decisions, artifacts and next steps. Verify the current environment before acting.\n\nGoal: {}\nRecord:\n{}",task.name,serde_json::to_string_pretty(&task.data)?)}),
                )
            }
            "sync.key_generate" => {
                crypto::generate_key(Path::new(text("path")?))?;
                Ok(json!({"created":true,"key_material_returned":false}))
            }
            "sync.configure" => sync::configure(&self.store, text("remote")?, text("key_file")?),
            "sync.run" => sync::configured(&self.store)?.run(&self.store),
            _ => Err(Error::new("unknown_method", "Unknown API method")),
        }
    }
}

/// Small validator for the schema subset used by the shared operation catalog.
fn validate(schema: &Value, params: &Value) -> Result<()> {
    let object = params
        .as_object()
        .ok_or_else(|| Error::new("invalid_params", "Parameters must be an object"))?;
    let properties = schema["properties"].as_object().expect("catalog schema");
    for required in schema["required"].as_array().expect("catalog required") {
        if !object.contains_key(required.as_str().expect("required field")) {
            return Err(Error::new(
                "invalid_params",
                format!("Missing required field: {required}"),
            ));
        }
    }
    for (key, value) in object {
        let spec = properties
            .get(key)
            .ok_or_else(|| Error::new("invalid_params", format!("Unknown field: {key}")))?;
        let type_ok = match spec["type"].as_str() {
            Some("string") => value.is_string() && !value.as_str().unwrap().is_empty(),
            Some("boolean") => value.is_boolean(),
            Some("object") => value.is_object(),
            Some("array") => value
                .as_array()
                .is_some_and(|a| a.len() >= 2 && a.iter().all(Value::is_string)),
            _ => true,
        };
        let enum_ok = spec
            .get("enum")
            .and_then(Value::as_array)
            .is_none_or(|values| values.contains(value));
        if !type_ok || !enum_ok {
            return Err(Error::new(
                "invalid_params",
                format!("Invalid value for {key}"),
            ));
        }
    }
    if serde_json::to_vec(params)?.len() > 1024 * 1024 {
        return Err(Error::new("input_too_large", "API input exceeds 1 MiB"));
    }
    Ok(())
}

pub fn envelope(result: Result<Value>) -> Value {
    match result {
        Ok(data) => json!({"api_version":"1","ok":true,"data":data}),
        Err(error) => json!({"api_version":"1","ok":false,"error":error}),
    }
}
