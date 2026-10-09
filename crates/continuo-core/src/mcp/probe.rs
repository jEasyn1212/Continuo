use super::Mapping;
use crate::{Error, Result};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
const LIMIT: u64 = 1024 * 1024;
const VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        self.0.stdin.take();
        std::thread::sleep(Duration::from_millis(15));
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGTERM);
        }
        #[cfg(not(unix))]
        {
            let _ = self.0.kill();
        }
        std::thread::sleep(Duration::from_millis(30));
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn send(child: &mut Child, value: Value) -> Result<()> {
    let stdin = child
        .stdin
        .as_mut()
        .ok_or_else(|| Error::new("mcp_probe_failed", "Connection input is closed"))?;
    writeln!(stdin, "{value}")
        .map_err(|_| Error::new("mcp_probe_failed", "Could not send protocol request"))?;
    stdin
        .flush()
        .map_err(|_| Error::new("mcp_probe_failed", "Could not flush protocol request"))?;
    Ok(())
}
fn receive(
    rx: &mpsc::Receiver<Result<Value>>,
    id: u64,
    deadline: Instant,
    cancel: &impl Fn() -> Result<bool>,
    child: &mut Child,
) -> Result<Value> {
    let mut frames = 0;
    let mut pings = 0;
    loop {
        if cancel()? {
            return Err(Error::new(
                "mcp_probe_cancelled",
                "Connection check was cancelled",
            ));
        }
        if Instant::now() >= deadline {
            return Err(Error::new(
                "mcp_probe_timeout",
                "Connection check reached its bounded timeout",
            ));
        }
        let message = match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(r) => r?,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => {
                return Err(Error::new(
                    "mcp_probe_failed",
                    "Server closed the protocol output",
                ))
            }
        };
        frames += 1;
        if frames > 128 {
            return Err(Error::new(
                "mcp_protocol_error",
                "Too many unrelated protocol frames",
            ));
        }
        if message["jsonrpc"] != "2.0" || !message.is_object() {
            return Err(Error::new(
                "mcp_protocol_error",
                "Server emitted an invalid JSON-RPC frame",
            ));
        }
        if message.get("method").is_some() {
            // Do not grant roots, sampling, elicitation, server-to-client tools or any other capability.
            if message["method"] == "ping"
                && message
                    .get("id")
                    .is_some_and(|id| id.is_string() || id.is_number())
            {
                pings += 1;
                if pings > 4 || message["id"].to_string().len() > 256 {
                    return Err(Error::new(
                        "mcp_protocol_error",
                        "Too many pings or an oversized ping identifier",
                    ));
                }
                send(
                    child,
                    json!({"jsonrpc":"2.0","id":message["id"],"result":{}}),
                )?;
                continue;
            }
            if message.get("id").is_some() {
                return Err(Error::new(
                    "mcp_protocol_error",
                    "Server requested an unsupported client capability",
                ));
            }
            continue;
        }
        if message["id"] != id {
            return Err(Error::new(
                "mcp_protocol_error",
                "Server responded with an unexpected request ID",
            ));
        }
        if message.get("error").is_some() {
            return Err(Error::new(
                "mcp_remote_error",
                "Server rejected the protocol request",
            ));
        }
        let result = message
            .get("result")
            .filter(|v| v.is_object())
            .ok_or_else(|| {
                Error::new("mcp_protocol_error", "Server response has no object result")
            })?;
        return Ok(result.clone());
    }
}
pub(super) fn run(
    mapping: &Mapping,
    timeout: u64,
    cancel: impl Fn() -> Result<bool>,
) -> Result<Value> {
    let mut command = Command::new(&mapping.executable);
    command
        .args(&mapping.args)
        .env_clear()
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = &mapping.cwd {
        command.current_dir(cwd);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    if cancel()? {
        return Err(Error::new(
            "mcp_probe_cancelled",
            "Check was cancelled before launch",
        ));
    }
    let mut child = Process(command.spawn().map_err(|_| {
        Error::new(
            "mcp_probe_failed",
            "Could not start the explicitly selected executable",
        )
    })?);
    let stdout = child.0.stdout.take().unwrap();
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let fd = stdout.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(Error::new(
                "mcp_probe_failed",
                "Could not bound the protocol reader",
            ));
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_reader = stop.clone();
    let (tx, rx) = mpsc::sync_channel(16);
    let reader = std::thread::spawn(move || {
        let mut stream = stdout;
        let mut pending = Vec::new();
        let mut searched = 0;
        let mut chunk = [0u8; 4096];
        while !stop_reader.load(Ordering::Relaxed) {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => pending.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(_) => {
                    let _ = tx.send(Err(Error::new(
                        "mcp_protocol_error",
                        "Could not read protocol output",
                    )));
                    break;
                }
            }
            loop {
                let Some(offset) = pending[searched..].iter().position(|b| *b == b'\n') else {
                    searched = pending.len();
                    break;
                };
                let end = searched + offset;
                if end as u64 > LIMIT {
                    let _ = tx.send(Err(Error::new(
                        "mcp_protocol_error",
                        "Protocol frame exceeds 1 MiB",
                    )));
                    return;
                }
                let parsed = serde_json::from_slice(&pending[..end]).map_err(|_| {
                    Error::new(
                        "mcp_protocol_error",
                        "Server stdout must contain newline-delimited JSON-RPC",
                    )
                });
                pending.drain(..=end);
                searched = 0;
                if tx.send(parsed).is_err() {
                    return;
                }
            }
            if pending.len() as u64 > LIMIT {
                let _ = tx.send(Err(Error::new(
                    "mcp_protocol_error",
                    "Protocol frame exceeds 1 MiB",
                )));
                return;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_millis(timeout);
    let mut active_request = 1;
    let result = (|| {
        send(
            &mut child.0,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":VERSIONS[0],"capabilities":{},"clientInfo":{"name":"continuo-connection-check","version":env!("CARGO_PKG_VERSION")}}}),
        )?;
        let init = receive(&rx, 1, deadline, &cancel, &mut child.0)?;
        if !init["protocolVersion"]
            .as_str()
            .is_some_and(|v| VERSIONS.contains(&v))
            || !init["capabilities"].is_object()
            || !init["serverInfo"]["name"].is_string()
            || !init["serverInfo"]["version"].is_string()
        {
            return Err(Error::new(
                "mcp_protocol_error",
                "Unsupported protocol version or invalid initialization response",
            ));
        }
        send(
            &mut child.0,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )?;
        let mut tool_count = None;
        let mut catalog_complete = true;
        if init["capabilities"].get("tools").is_some() {
            if !init["capabilities"]["tools"].is_object() {
                return Err(Error::new("mcp_protocol_error", "Invalid tools capability"));
            }
            active_request = 2;
            send(
                &mut child.0,
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
            )?;
            let list = receive(&rx, 2, deadline, &cancel, &mut child.0)?;
            let tools = list["tools"]
                .as_array()
                .ok_or_else(|| Error::new("mcp_protocol_error", "Invalid tool catalog"))?;
            if tools.len() > 512
                || tools.iter().any(|t| {
                    !t["name"]
                        .as_str()
                        .is_some_and(|n| !n.is_empty() && n.len() <= 256)
                        || t["inputSchema"]["type"] != "object"
                })
            {
                return Err(Error::new(
                    "mcp_protocol_error",
                    "Invalid or oversized tool catalog",
                ));
            }
            tool_count = Some(tools.len());
            catalog_complete = list.get("nextCursor").is_none();
        }
        Ok(
            json!({"protocol_version":init["protocolVersion"],"tools_available":tool_count,"catalog_complete":catalog_complete,"tools_called":false,"server_identity_verified":false,"raw_server_output_stored":false}),
        )
    })();
    if result
        .as_ref()
        .is_err_and(|e| ["mcp_probe_timeout", "mcp_probe_cancelled"].contains(&e.code.as_str()))
    {
        let _ = send(
            &mut child.0,
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":active_request,"reason":"Local bounded connection check ended"}}),
        );
    }
    drop(rx);
    stop.store(true, Ordering::Relaxed);
    drop(child);
    #[cfg(unix)]
    {
        let _ = reader.join();
    }
    #[cfg(not(unix))]
    {
        if reader.is_finished() {
            let _ = reader.join();
        }
    }
    result
}
