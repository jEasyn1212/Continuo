//! Device-local generated configuration homes. Never merge or rewrite a user's home.
use crate::{
    adapters::{AgentAdapter, LaunchRequest, ManagedConfig},
    mcp,
    store::{private_permissions, Store},
    Error, Result,
};
use rusqlite::{Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn active_key(agent: &str) -> String {
    format!("deployment_active:{agent}")
}
fn version_key(agent: &str, revision: &str) -> String {
    format!("deployment_version:{agent}:{revision}")
}
fn uuid(id: &str) -> Result<()> {
    if Uuid::parse_str(id).ok().is_none_or(|v| v.to_string() != id) {
        return Err(Error::new(
            "invalid_params",
            "Use a canonical revision UUID",
        ));
    }
    Ok(())
}
fn relative(path: &str) -> Result<()> {
    if path.is_empty()
        || Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(Error::new(
            "unsafe_managed_path",
            "Configuration path must stay within the generated home",
        ));
    }
    Ok(())
}
/// Check before touching directories. This is a configuration guard, not a same-user sandbox.
fn directory(path: &Path, create: bool) -> Result<()> {
    if path.exists() || fs::symlink_metadata(path).is_ok() {
        if !fs::symlink_metadata(path)?.is_dir() {
            return Err(Error::new(
                "unsafe_managed_path",
                "Managed directory is not a real directory",
            ));
        }
    } else if create {
        fs::create_dir(path)?;
        private_permissions(path, true)?;
    } else {
        return Err(Error::new(
            "managed_config_missing",
            "Generated configuration directory is missing",
        ));
    }
    Ok(())
}
fn root(store: &Store, agent: &str, create: bool) -> Result<PathBuf> {
    if !agent
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || agent.is_empty()
    {
        return Err(Error::new("invalid_adapter", "Invalid adapter identifier"));
    }
    directory(&store.data_dir, false)?;
    let mut path = fs::canonicalize(&store.data_dir)?;
    for part in ["managed-homes", agent, "generations"] {
        path.push(part);
        directory(&path, create)?;
    }
    Ok(path)
}
fn config_path(home: &Path, relative_path: &str, create: bool) -> Result<PathBuf> {
    relative(relative_path)?;
    let mut path = home.to_path_buf();
    let components: Vec<_> = Path::new(relative_path).components().collect();
    for component in &components[..components.len() - 1] {
        path.push(component.as_os_str());
        directory(&path, create)?;
    }
    path.push(components.last().unwrap().as_os_str());
    Ok(path)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    revision: String,
    generation: String,
    parent: Option<String>,
    action: String,
    definition_id: String,
    definition_revision: String,
    mapping_revision: String,
    relative_path: String,
    content_digest: String,
    environment_roots: BTreeMap<String, String>,
}
fn selection(store: &Store, agent: &str) -> Result<Option<Selection>> {
    store
        .metadata(&active_key(agent))?
        .map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}
fn verify(store: &Store, agent: &str, selection: &Selection) -> Result<PathBuf> {
    uuid(&selection.generation)?;
    let home = root(store, agent, false)?.join(&selection.generation);
    directory(&home, false)?;
    let path = config_path(&home, &selection.relative_path, false)?;
    let meta = fs::symlink_metadata(&path).map_err(|_| {
        Error::new(
            "managed_config_missing",
            "Generated configuration file is missing",
        )
    })?;
    if !meta.is_file() || meta.len() > 1024 * 1024 || {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            meta.nlink() != 1
        }
        #[cfg(not(unix))]
        {
            false
        }
    } {
        return Err(Error::new(
            "unsafe_managed_path",
            "Configuration is not a private regular file",
        ));
    }
    if digest(&fs::read(path)?) != selection.content_digest {
        return Err(Error::new(
            "managed_config_modified",
            "Generated configuration changed outside Continuo; refusing to replace or activate it",
        ));
    }
    for value in selection.environment_roots.values() {
        relative(value)?;
        directory(&home.join(value), false)?;
    }
    Ok(home)
}
pub fn inspect(store: &Store, adapter: &dyn AgentAdapter) -> Result<Value> {
    let agent = adapter.descriptor().id;
    let current = selection(store, agent)?;
    inspection(store, agent, current)
}
fn inspection(store: &Store, agent: &str, current: Option<Selection>) -> Result<Value> {
    let integrity = current.as_ref().map(|s| match verify(store, agent, s) {
        Ok(home) => json!({"ready":true,"home":home}),
        Err(e) => json!({"ready":false,"error":e}),
    });
    Ok(
        json!({"target_agent":agent,"scope":"device_generated_home","active":current,
        "expected_revision":current.as_ref().map(|s|s.revision.as_str()).unwrap_or("none"),
        "integrity":integrity,"personal_config_written":false,"account_authentication_verified":false,
        "security_isolation":false,"process_execution_supported":false,"credential_files_copied":false,"credentials_resolved":false}),
    )
}
pub fn plan(store: &Store, adapter: &dyn AgentAdapter, id: &str) -> Result<Value> {
    let prepared = mcp::prepare(store, id, None, adapter)?;
    let config: ManagedConfig = adapter.prepare_managed_config(&prepared["registration"])?;
    relative(&config.relative_path)?;
    if config.content.len() > 1024 * 1024
        || config.environment_roots.get("HOME").map(String::as_str) != Some(".")
    {
        return Err(Error::new(
            "managed_config_unsupported",
            "Generated configuration contract exceeds supported scope",
        ));
    }
    for (key, value) in &config.environment_roots {
        if !["HOME", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "HERMES_HOME"].contains(&key.as_str()) {
            return Err(Error::new(
                "managed_config_unsupported",
                "Only explicit configuration roots are supported",
            ));
        }
        relative(value)?;
    }
    let mut out = json!({"target_agent":adapter.descriptor().id,"definition_id":id,
        "definition_revision":prepared["definition_revision"],"mapping_revision":prepared["mapping_revision"],
        "file":config,"scope":"device_generated_home","personal_config_written":false,"executed":false});
    let fingerprint = digest(&serde_json::to_vec(&out)?);
    out["plan_digest"] = json!(fingerprint);
    Ok(out)
}
pub fn apply(
    store: &Store,
    adapter: &dyn AgentAdapter,
    id: &str,
    expected: &str,
    expected_digest: &str,
) -> Result<Value> {
    let proposed = plan(store, adapter, id)?;
    if proposed["plan_digest"] != expected_digest {
        return Err(Error::new(
            "deployment_plan_changed",
            "Review the current generated file before applying",
        ));
    }
    let agent = adapter.descriptor().id;
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    let previous = selection(store, agent)?;
    if previous
        .as_ref()
        .map(|s| s.revision.as_str())
        .unwrap_or("none")
        != expected
    {
        return Err(Error::new(
            "deployment_revision_conflict",
            "Managed configuration selection changed",
        ));
    }
    if let Some(previous) = &previous {
        verify(store, agent, previous)?;
    }
    let current = store.get(id)?;
    if current.conflicted
        || current.heads[0].deleted
        || current.heads[0].revision != proposed["definition_revision"]
        || mcp::mapping(store, id)?
            .map(|m| json!(m.revision))
            .unwrap_or(Value::Null)
            != proposed["mapping_revision"]
    {
        return Err(Error::new(
            "deployment_plan_changed",
            "MCP definition or device mapping changed",
        ));
    }
    let generations = root(store, agent, true)?;
    let staged = tempfile::tempdir_in(&generations)?;
    private_permissions(staged.path(), true)?;
    let file = &proposed["file"];
    for value in file["environment_roots"].as_object().unwrap().values() {
        let path = staged.path().join(value.as_str().unwrap());
        if path != staged.path() {
            // All built-in adapter roots are a single relative directory.
            if Path::new(value.as_str().unwrap()).components().count() != 1 {
                return Err(Error::new(
                    "managed_config_unsupported",
                    "Nested environment root is unsupported",
                ));
            }
            directory(&path, true)?;
        }
    }
    let path = config_path(staged.path(), file["relative_path"].as_str().unwrap(), true)?;
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut handle = options.open(&path)?;
    handle.write_all(file["content"].as_str().unwrap().as_bytes())?;
    handle.sync_all()?;
    let generation = Uuid::new_v4().to_string();
    let home = generations.join(&generation);
    #[cfg(unix)]
    {
        fs::File::open(path.parent().unwrap())?.sync_all()?;
        fs::File::open(staged.path())?.sync_all()?;
    }
    fs::rename(staged.path(), &home)?;
    #[cfg(unix)]
    fs::File::open(&generations)?.sync_all()?;
    let selected = Selection {
        revision: Uuid::new_v4().to_string(),
        generation,
        parent: previous.map(|s| s.revision),
        action: "apply".into(),
        definition_id: id.into(),
        definition_revision: proposed["definition_revision"].as_str().unwrap().into(),
        mapping_revision: proposed["mapping_revision"].as_str().unwrap().into(),
        relative_path: file["relative_path"].as_str().unwrap().into(),
        content_digest: digest(file["content"].as_str().unwrap().as_bytes()),
        environment_roots: serde_json::from_value(file["environment_roots"].clone())?,
    };
    verify(store, agent, &selected)?;
    save(store, agent, &selected)?;
    // Files are complete before this pointer becomes visible. A failed commit leaves an inactive generation.
    tx.commit()?;
    inspection(store, agent, Some(selected))
}
fn save(store: &Store, agent: &str, selected: &Selection) -> Result<()> {
    let value = serde_json::to_string(selected)?;
    store.set_metadata(&version_key(agent, &selected.revision), &value)?;
    store.set_metadata(&active_key(agent), &value)?;
    Ok(())
}
pub fn rollback(
    store: &Store,
    adapter: &dyn AgentAdapter,
    expected: &str,
    target: &str,
) -> Result<Value> {
    uuid(target)?;
    let agent = adapter.descriptor().id;
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    let current = selection(store, agent)?.ok_or_else(|| {
        Error::new(
            "managed_config_missing",
            "No active generated configuration",
        )
    })?;
    if current.revision != expected {
        return Err(Error::new(
            "deployment_revision_conflict",
            "Selection changed",
        ));
    }
    let previous_integrity_error = verify(store, agent, &current).err();
    let mut target: Selection = serde_json::from_str(
        &store
            .metadata(&version_key(agent, target))?
            .ok_or_else(|| {
                Error::new(
                    "deployment_version_missing",
                    "Unknown generated configuration version for this adapter",
                )
            })?,
    )?;
    verify(store, agent, &target)?;
    target.revision = Uuid::new_v4().to_string();
    target.parent = Some(current.revision);
    target.action = "rollback".into();
    save(store, agent, &target)?;
    tx.commit()?;
    // Re-select a complete old home, do not copy runtime state or stop any process.
    let mut result = inspection(store, agent, Some(target))?;
    result["previous_integrity_error"] = serde_json::to_value(previous_integrity_error)?;
    result["previous_files_retained"] = json!(true);
    Ok(result)
}
pub fn launch_plan(
    store: &Store,
    adapter: &dyn AgentAdapter,
    expected: &str,
    request: &LaunchRequest,
) -> Result<Value> {
    let current = selection(store, adapter.descriptor().id)?.ok_or_else(|| {
        Error::new(
            "managed_config_missing",
            "Apply a reviewed generated configuration first",
        )
    })?;
    if current.revision != expected {
        return Err(Error::new(
            "deployment_revision_conflict",
            "Selection changed",
        ));
    }
    let source = mcp::prepare(store, &current.definition_id, None, adapter)?;
    if source["definition_revision"] != current.definition_revision
        || source["mapping_revision"] != current.mapping_revision
    {
        return Err(Error::new(
            "deployment_source_changed",
            "The generated file is stale; review and apply the current MCP definition/mapping",
        ));
    }
    let home = verify(store, adapter.descriptor().id, &current)?;
    let mut launch = serde_json::to_value(adapter.prepare_launch(request)?)?;
    let mut env = BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
    for (key, path) in &current.environment_roots {
        env.insert(key.clone(), home.join(path).to_string_lossy().into_owned());
    }
    launch["env"] = json!(env);
    launch["inherit_env"] = json!(false);
    launch["deployment_revision"] = json!(current.revision);
    launch["credential_files_copied"] = json!(false);
    launch["credentials_resolved"] = json!(false);
    launch["account_authentication_verified"] = json!(false);
    launch["security_isolation"] = json!(false);
    launch["scope"] = json!("device_generated_home");
    launch["warnings"].as_array_mut().unwrap().push(json!("Generated home and a clean environment do not isolate OS keychains, project configuration, the filesystem or network. Real execution requires separate authorization."));
    Ok(launch)
}
