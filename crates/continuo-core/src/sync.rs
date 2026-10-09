use crate::{crypto, model::Event, store::Store, Error, Result};
mod control;
use control::Control;
use rusqlite::{Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};
use uuid::Uuid;

const BRANCH: &str = "continuo-sync";
const MAX_EVENT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitConfig {
    pub remote: String,
    pub key_file: String,
}

/// Storage providers own transport only. Causality and encryption stay in core.
pub trait SyncBackend {
    fn run(&self, store: &Store) -> Result<Value>;
}
pub struct GitBackend {
    pub config: GitConfig,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u32,
    key_id: String,
}

pub fn configure(store: &Store, remote: &str, key_file: &str) -> Result<Value> {
    configure_checked(store, remote, key_file, None)
}
pub fn configure_checked(
    store: &Store,
    remote: &str,
    key_file: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let _lease = control::lease(store)?;
    validate_remote(remote)?;
    let key_path = std::fs::canonicalize(key_file)?;
    let key = crypto::read_key(&key_path)?;
    let config = GitConfig {
        remote: remote.into(),
        key_file: key_path.to_string_lossy().into(),
    };
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    let current = config_revision(store)?;
    if expected.is_some_and(|e| e != current) {
        return Err(Error::new(
            "sync_config_changed",
            "Synchronization settings changed",
        ));
    }
    store.set_metadata("sync_config", &serde_json::to_string(&config)?)?;
    store.set_metadata("sync_config_revision", &Uuid::new_v4().to_string())?;
    store.set_metadata("sync_key_id", &crypto::key_id(&key))?;
    store.set_metadata("sync_enabled", "true")?;
    tx.commit()?;
    Ok(json!({"backend":"git","branch":BRANCH,"configured":true}))
}
pub fn configured(store: &Store) -> Result<GitBackend> {
    let raw = store.metadata("sync_config")?.ok_or_else(|| {
        Error::new(
            "sync_not_configured",
            "Configure a private Git repository and encryption key first",
        )
    })?;
    Ok(GitBackend {
        config: serde_json::from_str(&raw)?,
    })
}
fn validate_remote(remote: &str) -> Result<()> {
    if Path::new(remote).is_absolute() && Path::new(remote).is_dir() {
        return Ok(());
    }
    let repo = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("git@github.com:"));
    if let Some(repo) = repo {
        let parts: Vec<_> = repo.split('/').collect();
        if parts.len() == 2
            && parts.iter().all(|p| {
                !p.is_empty()
                    && *p != "."
                    && *p != ".."
                    && p.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            })
        {
            return Ok(());
        }
    }
    Err(Error::new("invalid_remote", "Use a GitHub HTTPS/SSH repository without embedded credentials, or a local bare repository for development"))
}

impl SyncBackend for GitBackend {
    fn run(&self, store: &Store) -> Result<Value> {
        self.controlled(store, None, false, 30000)
    }
}
impl GitBackend {
    pub fn controlled(
        &self,
        store: &Store,
        id: Option<&str>,
        preview: bool,
        timeout: u64,
    ) -> Result<Value> {
        let c = Control::begin(store, id, preview, timeout)?;
        let result = self.exchange(store, &c, preview);
        c.finish(&result)?;
        result
    }
    fn exchange(&self, store: &Store, c: &Control<'_>, preview: bool) -> Result<Value> {
        if Path::new(&self.config.remote).is_absolute() && !Path::new(&self.config.remote).is_dir()
        {
            return Err(Error::new(
                "git_failed",
                "Configured storage is unavailable",
            ));
        }
        validate_remote(&self.config.remote)?;
        if store.metadata("sync_enabled")?.as_deref() == Some("false") {
            return Err(Error::new(
                "sync_disabled",
                "Synchronization is paused on this device",
            ));
        }
        if configured(store)?.config != self.config {
            return Err(Error::new(
                "sync_config_changed",
                "Settings changed before synchronization",
            ));
        }
        let key = crypto::read_key(Path::new(&self.config.key_file)).map_err(|e| {
            if e.code == "io_error" {
                Error::new(
                    "sync_key_missing",
                    "Configured key is missing or unreadable",
                )
            } else {
                e
            }
        })?;
        if store
            .metadata("sync_key_id")?
            .is_some_and(|id| id != crypto::key_id(&key))
        {
            return Err(Error::new(
                "sync_key_changed",
                "The configured key file changed; restore the original key",
            ));
        }
        let workspace = workspace_id(&self.config.remote, &crypto::key_id(&key));
        c.phase("contacting")?;
        let scratch = tempfile::tempdir_in(&store.data_dir)?;
        let repo = scratch.path();
        c.git(repo, &["init", "--quiet"])?;
        c.git(repo, &["remote", "add", "origin", &self.config.remote])?;
        let remote_ref = format!("refs/heads/{BRANCH}");
        let refs = c.git(repo, &["ls-remote", "--heads", "origin", &remote_ref])?;
        let has_remote = !refs.trim().is_empty();
        let mut seen = BTreeSet::new();
        let mut remote_events: Vec<Event> = vec![];
        if has_remote {
            c.phase("fetching")?;
            c.git(
                repo,
                &["fetch", "--quiet", "--no-tags", "origin", &remote_ref],
            )?;
            c.phase("validating")?;
            let tree = c.git(repo, &["ls-tree", "-r", "-l", "FETCH_HEAD"])?;
            let entries = parse_tree(&tree)?;
            if !entries.iter().any(|(path, _)| path == "manifest.json") {
                return Err(Error::new(
                    "invalid_sync_repository",
                    "Sync branch has no manifest",
                ));
            }
            for (path, size) in &entries {
                c.check()?;
                if *size > MAX_EVENT_BYTES {
                    return Err(Error::new("sync_limit", "Remote event exceeds size limit"));
                }
                let bytes = c.bytes(repo, &["show", &format!("FETCH_HEAD:{path}")])?;
                if path == "manifest.json" {
                    let manifest: Manifest = serde_json::from_slice(&bytes)?;
                    if manifest.format != "continuo-events"
                        || manifest.version != 1
                        || manifest.key_id != crypto::key_id(&key)
                    {
                        return Err(Error::new(
                            "incompatible_workspace",
                            "Repository format or encryption key does not match",
                        ));
                    }
                } else {
                    let revision = path.trim_start_matches("events/").trim_end_matches(".json");
                    remote_events.push(crypto::open(&key, revision, &bytes)?);
                    seen.insert(revision.to_owned());
                }
            }
            // No checkout or smudge filters: only validated protocol blobs are materialized.
            c.git(repo, &["reset", "--soft", "FETCH_HEAD"])?;
            c.git(repo, &["read-tree", "FETCH_HEAD"])?;
            std::fs::create_dir(repo.join("events"))?;
            for (path, _) in entries {
                std::fs::write(
                    repo.join(&path),
                    c.bytes(repo, &["show", &format!("FETCH_HEAD:{path}")])?,
                )?;
            }
        } else {
            std::fs::create_dir(repo.join("events"))?;
        }
        if remote_events
            .iter()
            .any(|e| e.parents.iter().any(|p| !seen.contains(p)))
        {
            return Err(Error::new("missing_parent", "Remote history is incomplete"));
        }
        let downloaded = store.validate_import(&remote_events)?;
        if let Some(old) = checkpoint(store)? {
            if old.workspace_id == workspace && !old.known_revisions.is_subset(&seen) {
                return Err(Error::new(
                    "sync_remote_history_missing",
                    "Previously observed remote events disappeared; publishing is stopped",
                ));
            }
        }
        let remote_tip = if has_remote {
            Some(
                c.git(repo, &["rev-parse", "FETCH_HEAD"])?
                    .trim()
                    .to_string(),
            )
        } else {
            None
        };
        save_checkpoint(store, &workspace, &seen, remote_tip.clone())?;
        let before = store.events()?;
        let pending = before
            .iter()
            .filter(|e| !seen.contains(&e.revision))
            .count();
        if preview {
            return Ok(
                json!({"run_id":c.id,"preview_only":true,"downloaded":0,"uploaded":0,"pending_upload":pending,"pending_download":downloaded,"conflicts":store.list(None,false)?.iter().filter(|e|e.conflicted).count(),"published":false,"remote_tip":remote_tip}),
            );
        }
        c.phase("importing")?;
        let downloaded = store.import_events(&remote_events)?;
        c.phase("encrypting")?;
        let events = store.events()?;
        let mut uploaded = 0;
        for event in &events {
            c.check()?;
            event.validate()?;
            if !seen.contains(&event.revision) {
                std::fs::write(
                    repo.join("events").join(format!("{}.json", event.revision)),
                    crypto::seal(&key, event)?,
                )?;
                uploaded += 1;
            }
        }
        let manifest = Manifest {
            format: "continuo-events".into(),
            version: 1,
            key_id: crypto::key_id(&key),
        };
        std::fs::write(repo.join("manifest.json"), serde_json::to_vec(&manifest)?)?;
        c.git(repo, &["add", "--", "manifest.json", "events"])?;
        let changed = !has_remote
            || !c
                .output(repo, &["diff", "--cached", "--quiet"])?
                .status
                .success();
        if changed {
            c.git(
                repo,
                &[
                    "commit",
                    "--quiet",
                    "-m",
                    "Synchronize encrypted Continuo events",
                ],
            )?;
            c.phase("publishing")?;
            if crypto::key_id(&crypto::read_key(Path::new(&self.config.key_file))?)
                != crypto::key_id(&key)
            {
                return Err(Error::new(
                    "sync_key_changed",
                    "Key changed before publishing",
                ));
            }
            let pushed = c.output(
                repo,
                &["push", "--quiet", "origin", &format!("HEAD:{remote_ref}")],
            )?;
            if !pushed.status.success() {
                return Err(Error::new("sync_push_failed", "Remote changed or push failed; local changes are preserved. Run synchronization again to fetch and retry.").details(json!({"retry_safe":true,"downloaded":downloaded})));
            }
        }
        let acknowledged = events.iter().map(|e| e.revision.clone()).collect();
        let tip = if changed {
            Some(c.git(repo, &["rev-parse", "HEAD"])?.trim().to_string())
        } else {
            remote_tip
        };
        save_checkpoint(store, &workspace, &acknowledged, tip)?;
        let conflicts = store
            .list(None, false)?
            .iter()
            .filter(|e| e.conflicted)
            .count();
        let result = json!({"run_id":c.id,"backend":"git","branch":BRANCH,"downloaded":downloaded,"uploaded":uploaded,"conflicts":conflicts,"published":changed,"event_count":events.len()});
        store.set_metadata("last_sync", &serde_json::to_string(&result)?)?;
        Ok(result)
    }
}
fn parse_tree(tree: &str) -> Result<Vec<(String, u64)>> {
    let mut entries = vec![];
    for line in tree.lines() {
        let (meta, path) = line
            .split_once('\t')
            .ok_or_else(|| Error::new("invalid_sync_repository", "Invalid Git tree"))?;
        let fields: Vec<_> = meta.split_whitespace().collect();
        if fields.len() != 4 || fields[0] != "100644" || fields[1] != "blob" {
            return Err(Error::new(
                "invalid_sync_repository",
                "Only regular protocol files are permitted",
            ));
        }
        let valid = path == "manifest.json"
            || path
                .strip_prefix("events/")
                .and_then(|p| p.strip_suffix(".json"))
                .is_some_and(|p| {
                    Uuid::parse_str(p)
                        .ok()
                        .is_some_and(|id| id.to_string() == p)
                        && !p.contains('/')
                });
        if !valid {
            return Err(Error::new(
                "invalid_sync_repository",
                "Sync branch contains files outside the Continuo protocol",
            ));
        }
        let size = fields[3]
            .parse()
            .map_err(|_| Error::new("invalid_sync_repository", "Invalid blob size"))?;
        entries.push((path.to_owned(), size));
        if entries.len() > 50_000 {
            return Err(Error::new(
                "sync_limit",
                "Prototype supports at most 50,000 event files",
            ));
        }
    }
    Ok(entries)
}

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    workspace_id: String,
    known_revisions: BTreeSet<String>,
    remote_tip: Option<String>,
    checked_at_ms: u64,
}
fn workspace_id(remote: &str, key_id: &str) -> String {
    format!("{:x}", Sha256::digest(format!("{remote}\n{key_id}")))
}
fn checkpoint(store: &Store) -> Result<Option<Checkpoint>> {
    store
        .metadata("sync_checkpoint")?
        .map(|v| Ok(serde_json::from_str(&v)?))
        .transpose()
}
fn save_checkpoint(
    store: &Store,
    workspace: &str,
    seen: &BTreeSet<String>,
    tip: Option<String>,
) -> Result<()> {
    let value = Checkpoint {
        workspace_id: workspace.into(),
        known_revisions: seen.clone(),
        remote_tip: tip,
        checked_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    };
    store.set_metadata("sync_checkpoint", &serde_json::to_string(&value)?)
}
pub fn config_revision(store: &Store) -> Result<String> {
    Ok(store.metadata("sync_config_revision")?.unwrap_or(
        if store.metadata("sync_config")?.is_some() {
            "legacy"
        } else {
            "none"
        }
        .into(),
    ))
}
pub fn set_enabled(store: &Store, expected: &str, enabled: bool) -> Result<Value> {
    let _lease = control::lease(store)?;
    let cfg = configured(store)?.config;
    if enabled {
        let key = crypto::read_key(Path::new(&cfg.key_file))?;
        if store
            .metadata("sync_key_id")?
            .is_some_and(|id| id != crypto::key_id(&key))
        {
            return Err(Error::new(
                "sync_key_changed",
                "Restore the original configured key",
            ));
        }
    }
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    if config_revision(store)? != expected {
        return Err(Error::new("sync_config_changed", "Settings changed"));
    }
    store.set_metadata("sync_enabled", if enabled { "true" } else { "false" })?;
    let revision = Uuid::new_v4().to_string();
    store.set_metadata("sync_config_revision", &revision)?;
    tx.commit()?;
    Ok(
        json!({"enabled":enabled,"config_revision":revision,"device_revoked":false,"old_ciphertext_erased":false}),
    )
}
pub fn run(store: &Store, id: Option<&str>, preview: bool, timeout: u64) -> Result<Value> {
    configured(store)?.controlled(store, id, preview, timeout)
}
pub fn cancel(store: &Store, id: &str) -> Result<Value> {
    control::cancel(store, id)
}
pub fn job(store: &Store, id: &str) -> Result<Value> {
    control::job(store, id)
}
pub fn inspect(store: &Store) -> Result<Value> {
    let config = store
        .metadata("sync_config")?
        .map(|v| serde_json::from_str::<GitConfig>(&v))
        .transpose()?;
    let mut key_status = json!({"ready":false,"code":"not_configured"});
    let mut key_id = store.metadata("sync_key_id")?;
    if let Some(cfg) = &config {
        match crypto::read_key(Path::new(&cfg.key_file)) {
            Ok(key) => {
                let actual = crypto::key_id(&key);
                key_status = if key_id.as_ref().is_some_and(|id| id != &actual) {
                    json!({"ready":false,"code":"sync_key_changed"})
                } else {
                    json!({"ready":true})
                };
                if key_id.is_none() {
                    key_id = Some(actual);
                }
            }
            Err(e) => {
                key_status = json!({"ready":false,"code":if e.code=="io_error"{"sync_key_missing"}else{&e.code}})
            }
        }
    }
    let cp = checkpoint(store)?.filter(|cp| {
        config
            .as_ref()
            .zip(key_id.as_ref())
            .is_some_and(|(c, k)| cp.workspace_id == workspace_id(&c.remote, k))
    });
    let events = store.events()?;
    let ids: BTreeSet<_> = events.iter().map(|e| e.revision.clone()).collect();
    let pending_upload = cp
        .as_ref()
        .map(|cp| ids.difference(&cp.known_revisions).count());
    let pending_download = cp
        .as_ref()
        .map(|cp| cp.known_revisions.difference(&ids).count());
    let entities = store.list(None, true)?;
    let conflicts:Vec<_>=entities.iter().filter(|e|e.conflicted).map(|e|json!({"id":e.id,"kind":e.kind,"name":e.heads[0].name,"heads":e.heads.iter().map(|h|json!({"revision":h.revision,"deleted":h.deleted,"parents":h.parents,"device_id":h.device_id})).collect::<Vec<_>>()})).collect();
    Ok(
        json!({"configured":config.is_some(),"config":config,"config_revision":config_revision(store)?,"enabled":store.metadata("sync_enabled")?.as_deref()!=Some("false"),"key_status":key_status,"job":control::current(store)?,"last_success":store.metadata("last_sync")?.map(|v|serde_json::from_str::<Value>(&v)).transpose()?,"local_events":events.len(),"pending_upload":pending_upload,"pending_download":pending_download,"pending_basis":"last_validated_remote_snapshot_not_live","remote_tip":cp.as_ref().and_then(|c|c.remote_tip.clone()),"remote_checked_at_ms":cp.as_ref().map(|c|c.checked_at_ms),"conflicts":conflicts,"deleted_records":entities.iter().filter(|e|e.heads.iter().all(|h|h.deleted)).count(),"device_revocation_supported":false,"key_recovery_supported":false,"config_and_secrets_synchronized":false}),
    )
}
