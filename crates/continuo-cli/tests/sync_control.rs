#![cfg(unix)]
use continuo_core::{
    api::{Policy, Service},
    crypto, sync,
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn mcp_sync_workers_allow_cancellation_and_bounded_timeouts_without_diagnostics_on_stdout() {
    let fixture = tempfile::tempdir().unwrap();
    let remote = fixture.path().join("storage.git");
    assert!(Command::new("git")
        .args(["init", "--bare", "--quiet"])
        .arg(&remote)
        .status()
        .unwrap()
        .success());
    let key = fixture.path().join("key");
    crypto::generate_key(&key).unwrap();
    let service = Service::open(&fixture.path().join("vault"), Policy::local_user()).unwrap();
    sync::configure(
        &service.store,
        remote.to_str().unwrap(),
        key.to_str().unwrap(),
    )
    .unwrap();
    service
        .call(
            "entity.create",
            json!({"kind":"task","name":"Keep me","data":{}}),
        )
        .unwrap();
    let real = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real = String::from_utf8(real.stdout).unwrap();
    let wrapper_dir = fixture.path().join("wrapper");
    std::fs::create_dir(&wrapper_dir).unwrap();
    let wrapper = wrapper_dir.join("git");
    let escaped = real.trim().replace('\'', "'\\''");
    std::fs::write(&wrapper, format!("#!/bin/sh\nfor arg in \"$@\"; do\nif [ \"$arg\" = ls-remote ]; then /bin/sleep 10; break; fi\ndone\nexec '{escaped}' \"$@\"\n")).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!(
        "{}:{}",
        wrapper_dir.display(),
        std::env::var("PATH").unwrap()
    );
    let mut worker = Worker(
        Command::new(env!("CARGO_BIN_EXE_continuo"))
            .arg("--data-dir")
            .arg(&service.store.data_dir)
            .args(["mcp", "--allow-writes", "--allow-sync"])
            .env("PATH", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut input = worker.0.stdin.take().unwrap();
    let output = worker.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            sender
                .send(serde_json::from_str::<Value>(&line.unwrap()).unwrap())
                .unwrap();
        }
    });
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}})).unwrap();
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(3)).unwrap()["id"],
        1
    );
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    let run = uuid::Uuid::new_v4().to_string();
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"continuo_sync_run","arguments":{"run_id":run,"timeout_ms":5000}}})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while sync::inspect(&service.store).unwrap()["job"]["phase"] != "contacting" {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    // The main RPC loop must remain available while its transport worker is blocked.
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"continuo_sync_cancel","arguments":{"run_id":run}}})).unwrap();
    let mut responses = vec![
        receiver.recv_timeout(Duration::from_secs(3)).unwrap(),
        receiver.recv_timeout(Duration::from_secs(3)).unwrap(),
    ];
    responses.sort_by_key(|v| v["id"].as_u64());
    assert_eq!(
        responses[0]["result"]["structuredContent"]["error"]["code"],
        "sync_cancelled"
    );
    assert_eq!(
        responses[1]["result"]["structuredContent"]["data"]["cancel_requested"],
        true
    );
    assert_eq!(
        sync::job(&service.store, &run).unwrap()["state"],
        "cancelled"
    );
    assert_eq!(service.store.events().unwrap().len(), 1);
    let timed = uuid::Uuid::new_v4().to_string();
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"continuo_sync_preview","arguments":{"run_id":timed,"timeout_ms":150}}})).unwrap();
    let response = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(
        response["result"]["structuredContent"]["error"]["code"],
        "sync_timeout"
    );
    assert_eq!(
        sync::job(&service.store, &timed).unwrap()["publication"],
        "not_attempted"
    );
    assert_eq!(service.store.events().unwrap().len(), 1);
    drop(input);
    assert!(worker.0.wait().unwrap().success());
    reader.join().unwrap();
}

#[test]
fn timed_out_publication_is_unknown_and_retry_refetches_without_duplicate_events() {
    let fixture = tempfile::tempdir().unwrap();
    let remote = fixture.path().join("storage.git");
    assert!(Command::new("git")
        .args(["init", "--bare", "--quiet"])
        .arg(&remote)
        .status()
        .unwrap()
        .success());
    let key = fixture.path().join("key");
    crypto::generate_key(&key).unwrap();
    let service = Service::open(&fixture.path().join("vault"), Policy::local_user()).unwrap();
    sync::configure(
        &service.store,
        remote.to_str().unwrap(),
        key.to_str().unwrap(),
    )
    .unwrap();
    service
        .call(
            "entity.create",
            json!({"kind":"task","name":"Published before timeout","data":{}}),
        )
        .unwrap();
    let real = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real = String::from_utf8(real.stdout)
        .unwrap()
        .trim()
        .replace('\'', "'\\''");
    let wrapper_dir = fixture.path().join("wrapper");
    std::fs::create_dir(&wrapper_dir).unwrap();
    let wrapper = wrapper_dir.join("git");
    std::fs::write(&wrapper,format!("#!/bin/sh\nfor arg in \"$@\"; do\nif [ \"$arg\" = push ]; then\n'{real}' \"$@\"\nresult=$?\nif [ \"$result\" = 0 ]; then /bin/sleep 30; fi\nexit \"$result\"\nfi\ndone\nexec '{real}' \"$@\"\n")).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let run = uuid::Uuid::new_v4().to_string();
    let mut worker = Worker(
        Command::new(env!("CARGO_BIN_EXE_continuo"))
            .arg("--data-dir")
            .arg(&service.store.data_dir)
            .args(["--allow-sync", "call", "sync.run", "--input", "-"])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    wrapper_dir.display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut input = worker.0.stdin.take().unwrap();
    writeln!(input, "{}", json!({"run_id":run,"timeout_ms":5000})).unwrap();
    drop(input);
    let mut line = String::new();
    BufReader::new(worker.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let result: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(result["error"]["code"], "sync_timeout");
    assert!(!worker.0.wait().unwrap().success());
    assert_eq!(
        sync::job(&service.store, &run).unwrap()["publication"],
        "unknown"
    );
    let peer = Service::open(
        &fixture.path().join("peer"),
        Policy {
            sync: true,
            ..Policy::local_user()
        },
    )
    .unwrap();
    sync::configure(&peer.store, remote.to_str().unwrap(), key.to_str().unwrap()).unwrap();
    assert_eq!(peer.call("sync.run", json!({})).unwrap()["downloaded"], 1);
    let retry = sync::run(&service.store, None, false, 30000).unwrap();
    assert_eq!(retry["uploaded"], 0);
    assert_eq!(service.store.events().unwrap().len(), 1);
    assert_eq!(peer.store.events().unwrap().len(), 1);
}
