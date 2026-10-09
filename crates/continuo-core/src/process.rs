//! Local lifecycle proof using only the executable supplied by our interface.
//! No user executable, agent command, credential, prompt or portable event is executed.
use crate::{
    adapters::{AgentAdapter, LaunchRequest},
    deployment,
    store::Store,
    Error, Result,
};
use fs2::FileExt;
use rusqlite::{Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;

const LIMIT: usize = 4096;
#[derive(Serialize, Deserialize)]
struct Job {
    run_id: String,
    target_agent: String,
    deployment_revision: String,
    control_revision: String,
    state: String,
    stop_requested: bool,
    duration_ms: u64,
    timeout_ms: u64,
    exit_code_requested: u8,
    output_bytes: usize,
    ignore_stop: bool,
    env: Value,
    home: PathBuf,
    supervisor_pid: Option<u32>,
    worker_pid: Option<u32>,
    stdout: String,
    stderr: String,
    stdout_bytes: u64,
    stderr_bytes: u64,
    exit_code: Option<i32>,
    signal: Option<i32>,
    termination_verified: bool,
}
fn key(id: &str) -> String {
    format!("process_job:{id}")
}
fn active(agent: &str) -> String {
    format!("process_active:{agent}")
}
fn uuid(id: &str) -> Result<()> {
    if Uuid::parse_str(id).ok().is_none_or(|v| v.to_string() != id) {
        return Err(Error::new("invalid_params", "Use a canonical run UUID"));
    }
    Ok(())
}
fn get(store: &Store, id: &str) -> Result<Job> {
    uuid(id)?;
    serde_json::from_str(
        &store
            .metadata(&key(id))?
            .ok_or_else(|| Error::new("process_missing", "Run does not exist"))?,
    )
    .map_err(Into::into)
}
fn save(store: &Store, job: &Job) -> Result<()> {
    store.set_metadata(&key(&job.run_id), &serde_json::to_string(job)?)
}
fn terminal(state: &str) -> bool {
    matches!(
        state,
        "exited" | "stopped" | "timed_out" | "interrupted" | "failed"
    )
}
pub(crate) fn ensure_idle(store: &Store, agent: &str) -> Result<()> {
    if let Some(id) = store.metadata(&active(agent))? {
        if !terminal(&get(store, &id)?.state) {
            return Err(Error::new(
                "process_busy",
                "Stop or recover the existing run before changing its home or starting another",
            ));
        }
    }
    Ok(())
}
struct Lease(File);
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
fn lock(store: &Store, id: &str) -> Result<Option<Lease>> {
    uuid(id)?;
    let path = store.data_dir.join(format!("process-{id}.lock"));
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if !meta.is_file() {
            return Err(Error::new(
                "unsafe_managed_path",
                "Run lock must be a regular file",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() != 1 {
                return Err(Error::new(
                    "unsafe_managed_path",
                    "Run lock must be private",
                ));
            }
        }
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(Lease(file))),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn view(store: &Store, job: &Job) -> Result<Value> {
    let present = lock(store, &job.run_id)?.is_none();
    let mut value = serde_json::to_value(job)?;
    value["owner_present"] = json!(present);
    value["observed_state"] = json!(if !terminal(&job.state) && !present {
        "unowned"
    } else {
        &job.state
    });
    value["stdout_truncated"] = json!(job.stdout_bytes > LIMIT as u64);
    value["stderr_truncated"] = json!(job.stderr_bytes > LIMIT as u64);
    value["simulator_only"] = json!(true);
    value["security_isolation"] = json!(false);
    value["real_agent_executed"] = json!(false);
    value["portable"] = json!(false);
    Ok(value)
}
pub fn inspect(store: &Store, id: &str) -> Result<Value> {
    view(store, &get(store, id)?)
}
pub fn list(store: &Store) -> Result<Value> {
    let mut query = store.conn.prepare(
        "SELECT value FROM metadata WHERE key LIKE 'process_job:%' ORDER BY rowid DESC LIMIT 100",
    )?;
    let rows = query.query_map([], |r| r.get::<_, String>(0))?;
    let jobs = rows
        .map(|r| {
            let job: Job = serde_json::from_str(&r?)?;
            view(store, &job)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"runs":jobs,"limit":100,"scope":"device_local"}))
}
pub fn start(
    store: &Store,
    adapter: &dyn AgentAdapter,
    simulator: Option<&Path>,
    params: &Value,
) -> Result<Value> {
    if params["confirm_simulation"] != true {
        return Err(Error::new(
            "process_confirmation_required",
            "Explicit simulation confirmation is required",
        ));
    }
    #[cfg(not(unix))]
    return Err(Error::new(
        "process_platform_unsupported",
        "Managed simulation currently requires macOS/Linux",
    ));
    let binary = fs::canonicalize(simulator.ok_or_else(|| {
        Error::new(
            "process_unavailable",
            "Interface has no trusted built-in simulator",
        )
    })?)?;
    let id = params["run_id"].as_str().unwrap();
    uuid(id)?;
    let agent = adapter.descriptor().id;
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    ensure_idle(store, agent)?;
    if store.metadata(&key(id))?.is_some() {
        return Err(Error::new(
            "process_run_exists",
            "Run ID was already used; inspect it before retrying",
        ));
    }
    let selected = deployment::inspect(store, adapter)?;
    let home = PathBuf::from(selected["integrity"]["home"].as_str().ok_or_else(|| {
        Error::new(
            "managed_config_missing",
            "Apply a complete generated configuration first",
        )
    })?);
    let revision = params["expected_revision"].as_str().unwrap();
    let prepared = deployment::launch_plan(
        store,
        adapter,
        revision,
        &LaunchRequest {
            cwd: home.to_string_lossy().into_owned(),
            ..LaunchRequest::default()
        },
    )?;
    let job = Job {
        run_id: id.into(),
        target_agent: agent.into(),
        deployment_revision: revision.into(),
        control_revision: Uuid::new_v4().to_string(),
        state: "starting".into(),
        stop_requested: false,
        duration_ms: params["duration_ms"].as_u64().unwrap_or(1000),
        timeout_ms: params["timeout_ms"].as_u64().unwrap_or(5000),
        exit_code_requested: params["exit_code"].as_u64().unwrap_or(0) as u8,
        output_bytes: params["output_bytes"].as_u64().unwrap_or(256) as usize,
        ignore_stop: params["ignore_stop"].as_bool().unwrap_or(false),
        env: prepared["env"].clone(),
        home,
        supervisor_pid: None,
        worker_pid: None,
        stdout: String::new(),
        stderr: String::new(),
        stdout_bytes: 0,
        stderr_bytes: 0,
        exit_code: None,
        signal: None,
        termination_verified: false,
    };
    save(store, &job)?;
    store.set_metadata(&active(agent), id)?;
    tx.commit()?;
    let mut cmd = Command::new(binary);
    cmd.arg("--continuo-process-supervisor")
        .arg(fs::canonicalize(&store.data_dir)?)
        .arg(id)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(_) => {
            let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
            let mut job = get(store, id)?;
            job.state = "failed".into();
            job.control_revision = Uuid::new_v4().to_string();
            save(store, &job)?;
            tx.commit()?;
            return Err(Error::new(
                "process_spawn_failed",
                "Built-in supervisor could not start; inspect the run",
            ));
        }
    }
    inspect(store, id)
}
pub fn control(
    store: &Store,
    id: &str,
    revision: &str,
    confirmed: bool,
    recovery: bool,
) -> Result<Value> {
    if !confirmed {
        return Err(Error::new(
            "process_confirmation_required",
            "Confirm the exact run operation",
        ));
    }
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    let mut job = get(store, id)?;
    if job.control_revision != revision || terminal(&job.state) {
        return Err(Error::new(
            "process_revision_conflict",
            "Run changed or is already terminal; refresh",
        ));
    }
    let lease = lock(store, id)?;
    if recovery {
        if lease.is_none() {
            return Err(Error::new(
                "process_owner_present",
                "Supervisor still owns the run; request stop instead",
            ));
        }
        job.state = "interrupted".into(); // No saved PID is ever signalled or adopted.
    } else {
        job.stop_requested = true;
    }
    job.control_revision = Uuid::new_v4().to_string();
    save(store, &job)?;
    tx.commit()?;
    drop(lease);
    inspect(store, id)
}

/// Internal fixed roles handled before normal CLI or desktop startup. Never print protocol output.
pub fn internal_entry(args: &[String]) -> Option<Result<()>> {
    match args.first().map(String::as_str) {
        Some("--continuo-process-supervisor") => Some(if args.len() == 3 {
            supervisor(Path::new(&args[1]), &args[2])
        } else {
            Err(Error::new(
                "usage",
                "Invalid internal supervisor invocation",
            ))
        }),
        Some("--continuo-process-worker") => Some(worker(args)),
        _ => None,
    }
}
#[cfg(unix)]
fn nonblocking<T: std::os::fd::AsRawFd>(io: &T) -> Result<()> {
    let fd = io.as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
// Drop on every error path also terminates and reaps ONLY the child held by this owner.
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
#[cfg(unix)]
fn drain(reader: &mut impl Read, text: &mut String, total: &mut u64) -> Result<()> {
    let mut bytes = [0u8; 4096];
    for _ in 0..32 {
        match reader.read(&mut bytes) {
            Ok(0) => break,
            Ok(n) => {
                *total += n as u64;
                let remaining = LIMIT.saturating_sub(text.len());
                let n = n.min(remaining);
                text.push_str(&String::from_utf8_lossy(&bytes[..n]));
                while text.len() > LIMIT {
                    text.pop();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
#[cfg(unix)]
fn supervisor(path: &Path, id: &str) -> Result<()> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let store = Store::open(path)?;
    let Some(_lease) = lock(&store, id)? else {
        return Ok(());
    };
    let mut job;
    {
        let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
        job = get(&store, id)?;
        if job.state != "starting" {
            return Ok(());
        }
        if job.stop_requested {
            job.state = "stopped".into();
            job.termination_verified = true; // No child was created.
            job.control_revision = Uuid::new_v4().to_string();
            save(&store, &job)?;
            tx.commit()?;
            return Ok(());
        }
        job.state = "running".into();
        job.supervisor_pid = Some(std::process::id());
        save(&store, &job)?;
        tx.commit()?;
    }
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--continuo-process-worker",
            &job.duration_ms.to_string(),
            &job.exit_code_requested.to_string(),
            &job.output_bytes.to_string(),
            if job.ignore_stop { "true" } else { "false" },
        ])
        .env_clear()
        .current_dir(&job.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in job.env.as_object().unwrap() {
        command.env(name, value.as_str().unwrap());
    }
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = OwnedChild(command.spawn()?);
    let mut input = child.0.stdin.take().unwrap();
    let mut out = child.0.stdout.take().unwrap();
    let mut err = child.0.stderr.take().unwrap();
    nonblocking(&out)?;
    nonblocking(&err)?;
    let start = Instant::now();
    let mut stopping: Option<(Instant, &str)> = None;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut out_bytes = 0;
    let mut err_bytes = 0;
    loop {
        drain(&mut out, &mut stdout, &mut out_bytes)?;
        drain(&mut err, &mut stderr, &mut err_bytes)?;
        let status = child.0.try_wait()?;
        let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
        let mut current = get(&store, id)?;
        current.worker_pid = Some(child.0.id());
        current.stdout = stdout.clone();
        current.stderr = stderr.clone();
        current.stdout_bytes = out_bytes;
        current.stderr_bytes = err_bytes;
        if status.is_none()
            && stopping.is_none()
            && (current.stop_requested || start.elapsed() >= Duration::from_millis(job.timeout_ms))
        {
            let reason = if current.stop_requested {
                "stopped"
            } else {
                "timed_out"
            };
            let _ = input.write_all(b"stop\n");
            stopping = Some((Instant::now(), reason));
        }
        if let Some(status) = status {
            drain(&mut out, &mut current.stdout, &mut current.stdout_bytes)?;
            drain(&mut err, &mut current.stderr, &mut current.stderr_bytes)?;
            current.state = stopping
                .map(|(_, reason)| reason)
                .unwrap_or("exited")
                .into();
            current.exit_code = status.code();
            current.signal = status.signal();
            current.termination_verified = true;
            current.control_revision = Uuid::new_v4().to_string();
            save(&store, &current)?;
            tx.commit()?;
            break;
        }
        if let Some((since, _)) = stopping {
            // Only the unreaped direct child created above; never use a persisted PID.
            if since.elapsed() >= Duration::from_millis(300) {
                child.0.kill()?;
            } else if since.elapsed() >= Duration::from_millis(150) {
                unsafe {
                    libc::kill(-(child.0.id() as i32), libc::SIGTERM);
                }
            }
        }
        save(&store, &current)?;
        tx.commit()?;
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
#[cfg(not(unix))]
fn supervisor(_: &Path, _: &str) -> Result<()> {
    Err(Error::new(
        "process_platform_unsupported",
        "Managed simulation requires macOS/Linux",
    ))
}
#[cfg(unix)]
fn worker(args: &[String]) -> Result<()> {
    if args.len() != 5 {
        return Err(Error::new("usage", "Invalid internal worker invocation"));
    }
    let parse = |index: usize, max: u64| -> Result<u64> {
        let n = args[index]
            .parse::<u64>()
            .map_err(|_| Error::new("usage", "Invalid simulation bound"))?;
        if n > max {
            return Err(Error::new("usage", "Simulation exceeds bound"));
        }
        Ok(n)
    };
    let duration = parse(1, 10000)?;
    let code = parse(2, 125)?;
    let count = parse(3, 131072)? as usize;
    let mut input = std::io::stdin();
    nonblocking(&input)?;
    let started = Instant::now();
    let prefix = format!(
        "Continuo simulator only; HOME={}\n",
        std::env::var("HOME").unwrap_or_default()
    );
    std::io::stdout().write_all(prefix.as_bytes())?;
    std::io::stdout().write_all(&vec![b'x'; count])?;
    std::io::stderr().write_all(&vec![b'e'; count])?;
    let mut data = [0u8; 16];
    while started.elapsed() < Duration::from_millis(duration) {
        match input.read(&mut data) {
            Ok(0) => return Ok(()), // Supervisor loss always closes stdin, even for ignore_stop fixtures.
            Ok(_n) if args[4] != "true" => return Ok(()),
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.into()),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::process::exit(code as i32)
}
#[cfg(not(unix))]
fn worker(_: &[String]) -> Result<()> {
    Err(Error::new(
        "process_platform_unsupported",
        "Managed simulation requires macOS/Linux",
    ))
}
