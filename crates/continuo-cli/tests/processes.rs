#![cfg(unix)]
use continuo_core::api::{Policy, Service};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn fixture() -> (tempfile::TempDir, Service, Value) {
    let temp = tempfile::tempdir().unwrap();
    let s = Service::open(
        &temp.path().join("vault"),
        Policy {
            processes: true,
            ..Policy::local_user()
        },
    )
    .unwrap()
    .with_simulator(env!("CARGO_BIN_EXE_continuo").into());
    let e = s
        .call(
            "entity.create",
            json!({"kind":"mcp","name":"owned","data":{"transport":"stdio","server_key":"owned"}}),
        )
        .unwrap();
    s.call("mcp.map",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":"none","mapping":{"executable":env!("CARGO_BIN_EXE_continuo"),"args":["mcp"]}})).unwrap();
    (temp, s, e)
}
fn deploy(s: &Service, e: &Value, agent: &str, expected: &str) -> Value {
    let plan = s
        .call(
            "deployment.plan",
            json!({"target_agent":agent,"mcp_id":e["id"]}),
        )
        .unwrap();
    s.call("deployment.apply",json!({"target_agent":agent,"mcp_id":e["id"],"expected_revision":expected,"plan_digest":plan["plan_digest"],"confirm_generated_home":true})).unwrap()
}
fn request(d: &Value, agent: &str) -> Value {
    json!({"target_agent":agent,"expected_revision":d["expected_revision"],"run_id":uuid::Uuid::new_v4().to_string(),"confirm_simulation":true})
}
fn inspect(s: &Service, id: &Value) -> Value {
    s.call("process.inspect", json!({"run_id":id})).unwrap()
}
fn wait(s: &Service, id: &Value, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let job = inspect(s, id);
        if predicate(&job) {
            return job;
        }
        assert!(Instant::now() < deadline, "{job}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn end(s: &Service, id: &Value) -> Value {
    wait(s, id, |j| j["termination_verified"] == true)
}
#[test]
fn simulation_contract_all_three_homes_bounded_output_exit_codes_and_no_portable_runtime() {
    let (_temp, s, e) = fixture();
    for agent in ["claude-code", "codex", "hermes"] {
        let d = deploy(&s, &e, agent, "none");
        let mut p = request(&d, agent);
        p["exit_code"] = json!(7);
        p["output_bytes"] = json!(131072);
        p["duration_ms"] = json!(150);
        s.call("process.start", p.clone()).unwrap();
        let j = end(&s, &p["run_id"]);
        assert_eq!(j["state"], "exited");
        assert_eq!(j["exit_code"], 7);
        assert_eq!(j["real_agent_executed"], false);
        assert_eq!(j["security_isolation"], false);
        assert_eq!(j["stdout"].as_str().unwrap().len(), 4096);
        assert_eq!(j["stderr"].as_str().unwrap().len(), 4096);
        assert_eq!(j["stdout_truncated"], true);
        assert_eq!(j["stderr_truncated"], true);
        assert!(j["stdout"]
            .as_str()
            .unwrap()
            .contains(d["integrity"]["home"].as_str().unwrap()));
        assert_eq!(
            s.call("process.start", p).unwrap_err().code,
            "process_run_exists"
        );
    }
    assert_eq!(s.store.events().unwrap().len(), 1);
    let portable = serde_json::to_string(&s.store.events().unwrap()).unwrap();
    assert!(!portable.contains("process_job"));
    assert!(!portable.contains("managed-homes"));
}
#[test]
fn default_denial_confirmation_stale_selection_busy_and_stop_control_are_enforced() {
    let (_temp, s, e) = fixture();
    let d = deploy(&s, &e, "codex", "none");
    let mut p = request(&d, "codex");
    p["duration_ms"] = json!(10000);
    p["ignore_stop"] = json!(true);
    let denied = Service::open(&s.store.data_dir, Policy::local_user())
        .unwrap()
        .with_simulator(env!("CARGO_BIN_EXE_continuo").into());
    assert_eq!(
        denied.call("process.start", p.clone()).unwrap_err().code,
        "permission_denied"
    );
    p["confirm_simulation"] = json!(false);
    assert_eq!(
        s.call("process.start", p.clone()).unwrap_err().code,
        "process_confirmation_required"
    );
    p["confirm_simulation"] = json!(true);
    let mut stale = p.clone();
    stale["expected_revision"] = json!(uuid::Uuid::new_v4().to_string());
    assert_eq!(
        s.call("process.start", stale).unwrap_err().code,
        "deployment_revision_conflict"
    );
    s.call("process.start", p.clone()).unwrap();
    let running = wait(&s, &p["run_id"], |j| j["worker_pid"].is_number());
    assert_eq!(
        s.call("process.start", request(&d, "codex"))
            .unwrap_err()
            .code,
        "process_busy"
    );
    let plan = s
        .call(
            "deployment.plan",
            json!({"target_agent":"codex","mcp_id":e["id"]}),
        )
        .unwrap();
    assert_eq!(s.call("deployment.apply",json!({"target_agent":"codex","mcp_id":e["id"],"expected_revision":d["expected_revision"],"plan_digest":plan["plan_digest"],"confirm_generated_home":true})).unwrap_err().code,"process_busy");
    let control = json!({"run_id":p["run_id"],"expected_control_revision":running["control_revision"],"confirm_stop":true});
    s.call("process.stop", control.clone()).unwrap();
    assert_eq!(
        s.call("process.stop", control).unwrap_err().code,
        "process_revision_conflict"
    );
    let done = end(&s, &p["run_id"]);
    assert_eq!(done["state"], "stopped");
    assert_eq!(done["signal"], 15);
    let readonly = Service::open(&s.store.data_dir, Policy::read_only()).unwrap();
    assert_eq!(
        readonly.call("process.stop", json!({})).unwrap_err().code,
        "permission_denied"
    );
}
#[test]
fn timeout_and_owner_loss_recovery_never_adopt_or_signal_persisted_pid() {
    let (_temp, s, e) = fixture();
    let d = deploy(&s, &e, "codex", "none");
    let mut p = request(&d, "codex");
    p["duration_ms"] = json!(10000);
    p["timeout_ms"] = json!(100);
    p["ignore_stop"] = json!(true);
    s.call("process.start", p.clone()).unwrap();
    assert_eq!(end(&s, &p["run_id"])["state"], "timed_out");
    p = request(&d, "codex");
    p["duration_ms"] = json!(10000);
    s.call("process.start", p.clone()).unwrap();
    let j = wait(&s, &p["run_id"], |j| j["worker_pid"].is_number());
    let recovery = json!({"run_id":p["run_id"],"expected_control_revision":j["control_revision"],"confirm_recovery":true});
    assert_eq!(
        s.call("process.recover", recovery.clone())
            .unwrap_err()
            .code,
        "process_owner_present"
    );
    // This PID belongs to the supervisor just created in this isolated fixture, not user state.
    assert!(Command::new("/bin/kill")
        .args(["-KILL", &j["supervisor_pid"].to_string()])
        .status()
        .unwrap()
        .success());
    wait(&s, &p["run_id"], |j| j["owner_present"] == false);
    let mut stored: Value = serde_json::from_str(
        &s.store
            .metadata(&format!("process_job:{}", p["run_id"].as_str().unwrap()))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    stored["worker_pid"] = json!(std::process::id()); // Deliberate PID reuse fixture: must never kill this test process.
    s.store
        .set_metadata(
            &format!("process_job:{}", p["run_id"].as_str().unwrap()),
            &stored.to_string(),
        )
        .unwrap();
    let recovered = s.call("process.recover", recovery).unwrap();
    assert_eq!(recovered["state"], "interrupted");
    assert_eq!(recovered["termination_verified"], false);
    let fresh = request(&d, "codex");
    s.call("process.start", fresh.clone()).unwrap();
    assert_eq!(end(&s, &fresh["run_id"])["exit_code"], 0);
}
#[test]
fn mcp_parent_restart_keeps_supervisor_owned_and_cli_can_stop_with_clean_json() {
    let (_temp, s, e) = fixture();
    let d = deploy(&s, &e, "hermes", "none");
    let mut p = request(&d, "hermes");
    p["duration_ms"] = json!(10000);
    let mut parent = Command::new(env!("CARGO_BIN_EXE_continuo"))
        .arg("--data-dir")
        .arg(&s.store.data_dir)
        .args([
            "mcp",
            "--allow-writes",
            "--allow-admin",
            "--allow-managed-processes",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = parent.stdin.take().unwrap();
    let mut output = BufReader::new(parent.stdout.take().unwrap());
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}})).unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert!(serde_json::from_str::<Value>(&line).unwrap()["result"].is_object());
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"continuo_process_start","arguments":p}})).unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["result"]["structuredContent"]["ok"],
        true
    );
    let running = wait(&s, &p["run_id"], |j| j["worker_pid"].is_number());
    parent.kill().unwrap();
    parent.wait().unwrap();
    assert_eq!(inspect(&s, &p["run_id"])["owner_present"], true);
    let mut cli = Command::new(env!("CARGO_BIN_EXE_continuo"))
        .arg("--data-dir")
        .arg(&s.store.data_dir)
        .args(["call", "process.stop", "--input", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(cli.stdin.take().unwrap(),"{}",json!({"run_id":p["run_id"],"expected_control_revision":running["control_revision"],"confirm_stop":true})).unwrap();
    let stopped = cli.wait_with_output().unwrap();
    assert!(stopped.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&stopped.stdout).unwrap()["ok"],
        true
    );
    assert_eq!(end(&s, &p["run_id"])["state"], "stopped");
}
#[test]
fn built_in_worker_exits_when_supervisor_pipe_closes_even_when_stop_is_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_continuo"))
        .args(["--continuo-process-worker", "10000", "0", "0", "true"])
        .env_clear()
        .env("HOME", temp.path())
        .current_dir(temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    writeln!(pipe, "stop").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(child.try_wait().unwrap().is_none());
    drop(pipe);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("worker did not exit on supervisor loss");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn recovered_starting_record_blocks_a_delayed_supervisor_before_child_creation() {
    let (_temp, s, e) = fixture();
    let d = deploy(&s, &e, "codex", "none");
    let p = request(&d, "codex");
    s.call("process.start", p.clone()).unwrap();
    let done = end(&s, &p["run_id"]);
    wait(&s, &p["run_id"], |j| j["owner_present"] == false);
    let id = p["run_id"].as_str().unwrap();
    let k = format!("process_job:{id}");
    let mut record: Value = serde_json::from_str(&s.store.metadata(&k).unwrap().unwrap()).unwrap();
    record["state"] = json!("starting");
    record["termination_verified"] = json!(false);
    record["supervisor_pid"] = Value::Null;
    record["worker_pid"] = Value::Null;
    s.store.set_metadata(&k, &record.to_string()).unwrap();
    s.call("process.recover",json!({"run_id":id,"expected_control_revision":done["control_revision"],"confirm_recovery":true})).unwrap();
    let delayed = Command::new(env!("CARGO_BIN_EXE_continuo"))
        .arg("--continuo-process-supervisor")
        .arg(&s.store.data_dir)
        .arg(id)
        .output()
        .unwrap();
    assert!(delayed.status.success());
    assert!(delayed.stdout.is_empty());
    let final_record = inspect(&s, &p["run_id"]);
    assert_eq!(final_record["state"], "interrupted");
    assert!(final_record["worker_pid"].is_null());
}
