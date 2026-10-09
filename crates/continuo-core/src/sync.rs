use crate::{
    crypto,
    model::Event,
    store::{private_permissions, Store},
    Error, Result,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    path::Path,
    process::{Command, Output},
};
use uuid::Uuid;

const BRANCH: &str = "continuo-sync";
const MAX_EVENT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
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
    validate_remote(remote)?;
    let key_path = std::fs::canonicalize(key_file)?;
    crypto::read_key(&key_path)?;
    let config = GitConfig {
        remote: remote.into(),
        key_file: key_path.to_string_lossy().into(),
    };
    store.set_metadata("sync_config", &serde_json::to_string(&config)?)?;
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
        validate_remote(&self.config.remote)?;
        let lock_path = store.data_dir.join("sync.lock");
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;
        private_permissions(&lock_path, false)?;
        lock.try_lock_exclusive()
            .map_err(|_| Error::new("sync_busy", "Another synchronization is running"))?;
        let key = crypto::read_key(Path::new(&self.config.key_file))?;
        let scratch = tempfile::tempdir_in(&store.data_dir)?;
        let repo = scratch.path();
        git(repo, &["init", "--quiet"])?;
        git(repo, &["remote", "add", "origin", &self.config.remote])?;
        let remote_ref = format!("refs/heads/{BRANCH}");
        let refs = git(repo, &["ls-remote", "--heads", "origin", &remote_ref])?;
        let has_remote = !refs.trim().is_empty();
        let mut seen = BTreeSet::new();
        let mut remote_events: Vec<Event> = vec![];
        if has_remote {
            git(
                repo,
                &["fetch", "--quiet", "--no-tags", "origin", &remote_ref],
            )?;
            let tree = git(repo, &["ls-tree", "-r", "-l", "FETCH_HEAD"])?;
            let entries = parse_tree(&tree)?;
            if !entries.iter().any(|(path, _)| path == "manifest.json") {
                return Err(Error::new(
                    "invalid_sync_repository",
                    "Sync branch has no manifest",
                ));
            }
            for (path, size) in &entries {
                if *size > MAX_EVENT_BYTES {
                    return Err(Error::new("sync_limit", "Remote event exceeds size limit"));
                }
                let bytes = git_bytes(repo, &["show", &format!("FETCH_HEAD:{path}")])?;
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
            git(repo, &["reset", "--soft", "FETCH_HEAD"])?;
            git(repo, &["read-tree", "FETCH_HEAD"])?;
            std::fs::create_dir(repo.join("events"))?;
            for (path, _) in entries {
                std::fs::write(
                    repo.join(&path),
                    git_bytes(repo, &["show", &format!("FETCH_HEAD:{path}")])?,
                )?;
            }
        } else {
            std::fs::create_dir(repo.join("events"))?;
        }
        let downloaded = store.import_events(&remote_events)?;
        let events = store.events()?;
        let mut uploaded = 0;
        for event in &events {
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
        git(repo, &["add", "--", "manifest.json", "events"])?;
        let changed = !has_remote
            || !git_output(repo, &["diff", "--cached", "--quiet"])?
                .status
                .success();
        if changed {
            git(
                repo,
                &[
                    "commit",
                    "--quiet",
                    "-m",
                    "Synchronize encrypted Continuo events",
                ],
            )?;
            let pushed = git_output(
                repo,
                &["push", "--quiet", "origin", &format!("HEAD:{remote_ref}")],
            )?;
            if !pushed.status.success() {
                return Err(Error::new("sync_push_failed", "Remote changed or push failed; local changes are preserved. Run synchronization again to fetch and retry.").details(json!({"retry_safe":true,"downloaded":downloaded})));
            }
        }
        let conflicts = store
            .list(None, false)?
            .iter()
            .filter(|e| e.conflicted)
            .count();
        let result = json!({"backend":"git","branch":BRANCH,"downloaded":downloaded,"uploaded":uploaded,"conflicts":conflicts,"published":changed,"event_count":events.len()});
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
fn git_output(repo: &Path, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
        ])
        .args(args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Continuo")
        .env("GIT_COMMITTER_NAME", "Continuo")
        .env("GIT_AUTHOR_EMAIL", "continuo@users.noreply.github.com")
        .env("GIT_COMMITTER_EMAIL", "continuo@users.noreply.github.com");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(key);
    }
    Ok(command.output()?)
}
fn git_bytes(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = git_output(repo, args)?;
    if !output.status.success() {
        // Do not echo remote addresses or credential-helper diagnostics.
        return Err(Error::new(
            "git_failed",
            format!(
                "Git {} failed; check repository access and local Git authentication",
                args[0]
            ),
        ));
    }
    Ok(output.stdout)
}
fn git(repo: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(git_bytes(repo, args)?).map_err(|_| {
        Error::new(
            "invalid_sync_repository",
            "Git returned non-UTF-8 protocol metadata",
        )
    })
}
