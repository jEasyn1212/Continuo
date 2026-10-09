use crate::{
    store::{private_permissions, Store},
    Error, Result,
};
use fs2::FileExt;
use rusqlite::{Transaction, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
pub(super) struct SyncLease(File);
impl Drop for SyncLease {
    fn drop(&mut self) {
        // Closing alone can leave flock held by a descriptor inherited during another thread's fork.
        let _ = FileExt::unlock(&self.0);
    }
}
pub(super) fn lease(store: &Store) -> Result<SyncLease> {
    let path = store.data_dir.join("sync.lock");
    let f = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    private_permissions(&path, false)?;
    loop {
        match f.try_lock_exclusive() {
            Ok(()) => return Ok(SyncLease(f)),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(Error::new(
                    "sync_busy",
                    "Another synchronization is running",
                ))
            }
            Err(e) => return Err(e.into()),
        }
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn job_key(id: &str) -> String {
    format!("sync_job:{id}")
}
pub fn job(store: &Store, id: &str) -> Result<Value> {
    let raw = store
        .metadata(&job_key(id))?
        .ok_or_else(|| Error::new("sync_job_missing", "Synchronization job was not found"))?;
    let mut v: Value = serde_json::from_str(&raw)?;
    if v["state"] == "running"
        && (store.metadata("sync_active_job")?.as_deref() != Some(id) || lease(store).is_ok())
    {
        v["state"] = json!("interrupted");
        v["retry_safe"] = json!(true);
    }
    Ok(v)
}
pub fn current(store: &Store) -> Result<Value> {
    store
        .metadata("sync_active_job")?
        .map(|id| job(store, &id))
        .transpose()
        .map(|v| v.unwrap_or(Value::Null))
}
pub fn cancel(store: &Store, id: &str) -> Result<Value> {
    if lease(store).is_ok() {
        return Err(Error::new(
            "sync_job_changed",
            "No synchronization is currently running",
        ));
    }
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    if store.metadata("sync_active_job")?.as_deref() != Some(id) {
        return Err(Error::new(
            "sync_job_changed",
            "Choose the current running synchronization",
        ));
    }
    let mut v: Value = serde_json::from_str(
        &store
            .metadata(&job_key(id))?
            .ok_or_else(|| Error::new("sync_job_missing", "Job not found"))?,
    )?;
    if v["state"] != "running" {
        return Err(Error::new(
            "sync_job_changed",
            "Synchronization already ended",
        ));
    }
    v["cancel_requested"] = json!(true);
    store.set_metadata(&job_key(id), &v.to_string())?;
    tx.commit()?;
    Ok(json!({"run_id":id,"cancel_requested":true,"publication_may_have_completed":true}))
}
pub(super) struct Control<'a> {
    pub store: &'a Store,
    pub id: String,
    deadline: Instant,
    _lease: SyncLease,
}
impl<'a> Control<'a> {
    pub fn begin(store: &'a Store, id: Option<&str>, preview: bool, timeout: u64) -> Result<Self> {
        let id = id
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        if Uuid::parse_str(&id)
            .ok()
            .is_none_or(|v| v.to_string() != id)
            || !(100..=90000).contains(&timeout)
        {
            return Err(Error::new(
                "invalid_params",
                "Use a new canonical run UUID and timeout 100..90000 ms",
            ));
        }
        let l = lease(store)?;
        let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
        if store.metadata(&job_key(&id))?.is_some() {
            return Err(Error::new(
                "sync_job_exists",
                "Job UUID already exists; read it before starting a new retry",
            ));
        }
        let v = json!({"run_id":id,"mode":if preview{"preview"}else{"exchange"},"state":"running","phase":"checking","cancel_requested":false,"started_at_ms":now(),"publication":"not_attempted","retry_safe":true});
        store.set_metadata(&job_key(&id), &v.to_string())?;
        store.set_metadata("sync_active_job", &id)?;
        tx.commit()?;
        Ok(Self {
            store,
            id,
            deadline: Instant::now() + Duration::from_millis(timeout),
            _lease: l,
        })
    }
    pub fn check(&self) -> Result<()> {
        if Instant::now() >= self.deadline {
            return Err(Error::new(
                "sync_timeout",
                "Synchronization reached its bounded timeout",
            ));
        }
        let v: Value =
            serde_json::from_str(&self.store.metadata(&job_key(&self.id))?.unwrap_or_default())?;
        if v["cancel_requested"] == true {
            return Err(Error::new("sync_cancelled", "Cancellation was requested"));
        }
        Ok(())
    }
    pub fn phase(&self, phase: &str) -> Result<()> {
        self.check()?;
        let tx = Transaction::new_unchecked(&self.store.conn, TransactionBehavior::Immediate)?;
        let mut v: Value =
            serde_json::from_str(&self.store.metadata(&job_key(&self.id))?.unwrap_or_default())?;
        v["phase"] = json!(phase);
        if phase == "publishing" {
            v["publication"] = json!("unknown");
        }
        self.store
            .set_metadata(&job_key(&self.id), &v.to_string())?;
        tx.commit()?;
        Ok(())
    }
    pub fn finish(&self, result: &Result<Value>) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.store.conn, TransactionBehavior::Immediate)?;
        let mut v: Value =
            serde_json::from_str(&self.store.metadata(&job_key(&self.id))?.unwrap_or_default())?;
        v["ended_at_ms"] = json!(now());
        match result {
            Ok(r) => {
                v["state"] = json!("succeeded");
                v["result"] = r.clone();
                if r["published"] == true {
                    v["publication"] = json!("confirmed");
                }
            }
            Err(e) => {
                v["state"] = json!(match e.code.as_str() {
                    "sync_cancelled" => "cancelled",
                    "sync_timeout" => "timeout",
                    _ => "failed",
                });
                v["error"] = json!({"code":e.code});
            }
        }
        self.store
            .set_metadata(&job_key(&self.id), &v.to_string())?;
        tx.commit()?;
        Ok(())
    }
    pub fn output(&self, repo: &Path, args: &[&str]) -> Result<Output> {
        #[cfg(not(unix))]
        {
            let _ = (repo, args);
            return Err(Error::new(
                "sync_transport_unsupported",
                "Bounded Git transport currently supports macOS/Linux",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::process::CommandExt};
            self.check()?;
            let mut cmd = Command::new("git");
            cmd.args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .args(args)
            .current_dir(repo)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "/usr/bin/false")
            .env("SSH_ASKPASS", "/usr/bin/false")
            .env("GIT_AUTHOR_NAME", "Continuo")
            .env("GIT_COMMITTER_NAME", "Continuo")
            .env("GIT_AUTHOR_EMAIL", "continuo@users.noreply.github.com")
            .env("GIT_COMMITTER_EMAIL", "continuo@users.noreply.github.com")
            .process_group(0);
            for key in [
                "GIT_DIR",
                "GIT_WORK_TREE",
                "GIT_INDEX_FILE",
                "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            ] {
                cmd.env_remove(key);
            }
            let mut child = Process(
                cmd.spawn()
                    .map_err(|_| Error::new("git_unavailable", "Could not start Git"))?,
                false,
            );
            let mut stdout = child.0.stdout.take().unwrap();
            let fd = stdout.as_raw_fd();
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(Error::new("git_failed", "Could not bound Git output"));
            }
            let mut bytes = vec![];
            let mut buffer = [0u8; 8192];
            let mut eof = false;
            let mut status = None;
            loop {
                self.check()?;
                match stdout.read(&mut buffer) {
                    Ok(0) => eof = true,
                    Ok(n) => {
                        bytes.extend_from_slice(&buffer[..n]);
                        if bytes.len() > 20 * 1024 * 1024 {
                            return Err(Error::new("sync_limit", "Git response exceeds limit"));
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return Err(Error::new("git_failed", "Could not read Git output")),
                }
                if status.is_none() {
                    status = child.0.try_wait()?;
                    if status.is_some() {
                        child.1 = true;
                    }
                }
                if eof {
                    if let Some(status) = status {
                        return Ok(Output {
                            status,
                            stdout: bytes,
                            stderr: vec![],
                        });
                    }
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    pub fn bytes(&self, repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
        let o = self.output(repo, args)?;
        if !o.status.success() {
            return Err(Error::new(
                "git_failed",
                "Git operation failed; check repository access and local authentication",
            ));
        }
        Ok(o.stdout)
    }
    pub fn git(&self, repo: &Path, args: &[&str]) -> Result<String> {
        String::from_utf8(self.bytes(repo, args)?).map_err(|_| {
            Error::new(
                "invalid_sync_repository",
                "Git returned invalid protocol metadata",
            )
        })
    }
}
struct Process(Child, bool);
impl Drop for Process {
    fn drop(&mut self) {
        if self.1 {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGTERM);
        }
        thread::sleep(Duration::from_millis(15));
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lease_release_is_not_delayed_by_an_inherited_file_description() {
        let t = tempfile::tempdir().unwrap();
        let store = Store::open(t.path()).unwrap();
        let held = lease(&store).unwrap();
        let inherited = held.0.try_clone().unwrap();
        assert!(lease(&store).is_err());
        drop(held);
        assert!(lease(&store).is_ok(), "completed owner's lease must release even while a fork-like descriptor copy still exists");
        drop(inherited);
    }
}
