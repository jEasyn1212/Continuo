use continuo_core::{
    api::{Policy, Service},
    crypto,
    sync::{configure, configured, SyncBackend},
};
use serde_json::{json, Value};
use std::{path::Path, process::Command, thread, time::Duration};
fn fixture(root: &Path, mode: &str) -> (String, Vec<String>) {
    let output = Command::new("sh")
        .args(["-c", "command -v python3"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let python = String::from_utf8(output.stdout).unwrap().trim().to_string();
    let file = root.join(format!("server-{mode}.py"));
    std::fs::write(&file,r#"import json, sys, time
mode=sys.argv[1]
for line in sys.stdin:
 r=json.loads(line)
 if mode=='hang': time.sleep(10); continue
 if mode=='oversized': print('x'*1100000,flush=True);continue
 if mode=='request': print(json.dumps({'jsonrpc':'2.0','id':999,'method':'sampling/createMessage'}),flush=True);continue
 if mode=='ping-flood':
  for n in range(8): print(json.dumps({'jsonrpc':'2.0','id':n,'method':'ping'}),flush=True)
  continue
 if mode=='ping-large-id': print(json.dumps({'jsonrpc':'2.0','id':'x'*1000,'method':'ping'}),flush=True);continue
 if 'id' not in r: continue
 if mode=='malformed': print('PRIVATE RAW ERROR SHOULD NOT LEAK',flush=True); continue
 if mode=='error': print(json.dumps({'jsonrpc':'2.0','id':r['id'],'error':{'code':-32603,'message':'SECRET RAW ERROR'}}),flush=True);continue
 if r['method']=='initialize':
  result={'protocolVersion':'2099-invalid' if mode=='version' else '2025-11-25','serverInfo':{'name':'isolated fixture','version':'1'},'capabilities':{'tools':{}}}
 else: result={'tools':[{'name':'fixture_echo','inputSchema':{'type':'object'}}]}
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
"#).unwrap();
    (python, vec![file.to_string_lossy().into(), mode.into()])
}
fn create(s: &Service) -> Value {
    s.call("entity.create",json!({"kind":"mcp","name":"Fixture MCP","data":{"server_key":"fixture-server","command_hint":"python3"}})).unwrap()
}
fn mapping(s: &Service, e: &Value, exe: &str, args: &[String], expected: &str) -> Value {
    s.call("mcp.map",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":expected,"mapping":{"executable":exe,"args":args}})).unwrap()["mapping"].clone()
}
fn probe(s: &Service, e: &Value, m: &Value, id: &str, timeout: u64) -> Value {
    s.call("mcp.probe",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":m["revision"],"probe_id":id,"confirm_execution":true,"timeout_ms":timeout})).unwrap()
}
#[test]
fn real_isolated_stdio_handshake_and_registration_keep_definition_mapping_connection_and_authorization_distinct(
) {
    let t = tempfile::tempdir().unwrap();
    let s = Service::open(
        &t.path().join("vault"),
        Policy {
            probes: true,
            ..Policy::local_user()
        },
    )
    .unwrap();
    let e = create(&s);
    let i = s
        .call("mcp.inspect", json!({"id":e["id"],"target_agent":"codex"}))
        .unwrap();
    assert_eq!(i["ready"], false);
    assert_eq!(i["connection"]["state"], "not_checked");
    assert_eq!(i["authorization"]["persistent_permissions_granted"], false);
    let (exe, mut args) = fixture(t.path(), "ok");
    args.push("Literal $(do-not-run)\n\"quoted argument\"".into());
    let m = mapping(&s, &e, &exe, &args, "none");
    let checked = probe(&s, &e, &m, &uuid::Uuid::new_v4().to_string(), 1000);
    assert_eq!(checked["state"], "succeeded");
    assert_eq!(checked["result"]["tools_available"], 1);
    assert_eq!(checked["tools_called"], false);
    for agent in ["claude-code", "codex", "hermes"] {
        let p = s
            .call("mcp.prepare", json!({"id":e["id"],"target_agent":agent}))
            .unwrap();
        let doc = &p["registration"]["document"];
        let entry = if agent == "claude-code" {
            &doc["mcpServers"]["fixture-server"]
        } else {
            &doc["mcp_servers"]["fixture-server"]
        };
        assert_eq!(entry["command"], exe);
        assert_eq!(entry["args"], json!(args));
        let native = p["registration"]["native_text"].as_str().unwrap();
        if agent == "codex" {
            let parsed: toml::Value = toml::from_str(native).unwrap();
            assert_eq!(
                parsed["mcp_servers"]["fixture-server"]["command"].as_str(),
                Some(exe.as_str())
            );
            assert_eq!(
                parsed["mcp_servers"]["fixture-server"]["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect::<Vec<_>>(),
                args.iter().map(String::as_str).collect::<Vec<_>>()
            );
        } else {
            let parsed: Value = serde_json::from_str(native).unwrap();
            assert_eq!(parsed, p["registration"]["document"]);
        }
        assert_eq!(p["config_written"], false);
        assert_eq!(p["permissions_granted"], false);
    }
    let local = s
        .call("mcp.inspect", json!({"id":e["id"],"target_agent":"codex"}))
        .unwrap();
    assert_eq!(local["connection"]["state"], "succeeded");
    let id = s
        .call(
            "entity.create",
            json!({"kind":"identity","name":"Tools","data":{"mcp_ids":[e["id"]]}}),
        )
        .unwrap();
    let p = s
        .call(
            "agent.prepare",
            json!({"agent":"codex","cwd":"/tmp","identity_id":id["id"]}),
        )
        .unwrap();
    assert_eq!(p["mcp_context"][0]["registration"]["state"], "prepared");
    assert_eq!(p["mcp_context"][0]["installed"], false);
    let denied = Service::open(&t.path().join("vault"), Policy::local_user()).unwrap();
    assert_eq!(denied.call("mcp.probe",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":m["revision"],"probe_id":uuid::Uuid::new_v4().to_string(),"confirm_execution":true})).unwrap_err().code,"permission_denied");
    assert!(!denied.available().iter().any(|o| o.method == "mcp.probe"));
    assert_eq!(s.call("mcp.probe",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":m["revision"],"probe_id":uuid::Uuid::new_v4().to_string(),"confirm_execution":false})).unwrap_err().code,"mcp_confirmation_required");
}
#[test]
fn malformed_remote_error_version_mismatch_and_timeout_do_not_leak_raw_output_and_can_be_retried() {
    let t = tempfile::tempdir().unwrap();
    let s = Service::open(
        &t.path().join("vault"),
        Policy {
            probes: true,
            ..Policy::local_user()
        },
    )
    .unwrap();
    let e = create(&s);
    let mut expected = "none".to_string();
    for (mode, status) in [
        ("malformed", "failed"),
        ("error", "failed"),
        ("version", "failed"),
        ("oversized", "failed"),
        ("request", "failed"),
        ("ping-flood", "failed"),
        ("ping-large-id", "failed"),
        ("hang", "timeout"),
        ("ok", "succeeded"),
    ] {
        let (exe, args) = fixture(t.path(), mode);
        let m = mapping(&s, &e, &exe, &args, &expected);
        expected = m["revision"].as_str().unwrap().into();
        let result = probe(
            &s,
            &e,
            &m,
            &uuid::Uuid::new_v4().to_string(),
            if mode == "hang" { 200 } else { 1000 },
        );
        assert_eq!(result["state"], status, "fixture mode {mode}");
        assert!(!result.to_string().contains("SECRET RAW"));
        assert!(!result.to_string().contains("PRIVATE RAW"));
    }
}
#[test]
fn cancellation_is_revision_bound_busy_checks_are_rejected_and_mapping_tombstones_prevent_aba() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("vault");
    let s = Service::open(
        &path,
        Policy {
            probes: true,
            ..Policy::local_user()
        },
    )
    .unwrap();
    let e = create(&s);
    let (exe, args) = fixture(t.path(), "hang");
    let m = mapping(&s, &e, &exe, &args, "none");
    let id = uuid::Uuid::new_v4().to_string();
    let (ee, mm, ii, pp) = (e.clone(), m.clone(), id.clone(), path.clone());
    let worker = thread::spawn(move || {
        let s = Service::open(
            &pp,
            Policy {
                probes: true,
                ..Policy::local_user()
            },
        )
        .unwrap();
        probe(&s, &ee, &mm, &ii, 2000)
    });
    for _ in 0..100 {
        if s.call("mcp.inspect", json!({"id":e["id"],"target_agent":"codex"}))
            .unwrap()["connection"]["state"]
            == "running"
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(s.call("mcp.probe",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":m["revision"],"probe_id":uuid::Uuid::new_v4().to_string(),"confirm_execution":true})).unwrap_err().code,"mcp_busy");
    assert_eq!(
        s.call(
            "mcp.cancel",
            json!({"id":e["id"],"probe_id":uuid::Uuid::new_v4().to_string()})
        )
        .unwrap_err()
        .code,
        "mcp_probe_conflict"
    );
    s.call("mcp.cancel", json!({"id":e["id"],"probe_id":id}))
        .unwrap();
    assert_eq!(worker.join().unwrap()["state"], "cancelled");
    s.call(
        "mcp.clear_mapping",
        json!({"id":e["id"],"expected_mapping_revision":m["revision"]}),
    )
    .unwrap();
    let i = s
        .call("mcp.inspect", json!({"id":e["id"],"target_agent":"codex"}))
        .unwrap();
    assert_eq!(i["connection"]["state"], "stale");
    assert_eq!(i["mapping"]["cleared"], true);
    assert_eq!(s.call("mcp.map",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":"none","mapping":{"executable":exe,"args":args}})).unwrap_err().code,"mcp_mapping_conflict");
    let m = mapping(
        &s,
        &e,
        &exe,
        &args,
        i["mapping"]["revision"].as_str().unwrap(),
    );
    s.call("entity.update",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"name":"Edited definition"})).unwrap();
    assert_eq!(s.call("mcp.probe",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":m["revision"],"probe_id":uuid::Uuid::new_v4().to_string(),"confirm_execution":true})).unwrap_err().code,"mcp_mapping_conflict");
}
#[test]
fn encrypted_sync_excludes_paths_mappings_and_probe_results_and_keeps_definition_conflicts() {
    let t = tempfile::tempdir().unwrap();
    let remote = t.path().join("remote.git");
    assert!(Command::new("git")
        .args(["init", "--bare", "--quiet"])
        .arg(&remote)
        .status()
        .unwrap()
        .success());
    let key = t.path().join("key");
    crypto::generate_key(&key).unwrap();
    let a = Service::open(&t.path().join("a"), Policy::local_user()).unwrap();
    let b = Service::open(&t.path().join("b"), Policy::local_user()).unwrap();
    for s in [&a, &b] {
        configure(&s.store, remote.to_str().unwrap(), key.to_str().unwrap()).unwrap();
    }
    let e = create(&a);
    let (exe, args) = fixture(t.path(), "ok");
    mapping(&a, &e, &exe, &args, "none");
    configured(&a.store).unwrap().run(&a.store).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    let i = b
        .call("mcp.inspect", json!({"id":e["id"],"target_agent":"codex"}))
        .unwrap();
    assert!(i["mapping"].is_null());
    assert_eq!(i["connection"]["state"], "not_checked");
    assert!(!serde_json::to_string(&a.store.events().unwrap())
        .unwrap()
        .contains(t.path().to_str().unwrap()));
    for (s, name) in [(&a, "A"), (&b, "B")] {
        s.call(
            "entity.update",
            json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"name":name}),
        )
        .unwrap();
    }
    configured(&a.store).unwrap().run(&a.store).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    assert_eq!(
        b.call("mcp.prepare", json!({"id":e["id"],"target_agent":"codex"}))
            .unwrap_err()
            .code,
        "mcp_unavailable"
    );
    for bad in [
        json!({"server_key":"x","command_hint":"/Users/private/executable"}),
        json!({"transport":"http","endpoint":"https://user:secret@host/mcp"}),
        json!({"required_env":["BAD=VALUE"]}),
    ] {
        assert!(a
            .call(
                "entity.create",
                json!({"kind":"mcp","name":"Bad","data":bad})
            )
            .is_err());
    }
    let c = create(&a);
    let m = mapping(&a, &c, &exe, &args, "none");
    let m=a.call("mcp.map",json!({"id":c["id"],"expected_revision":c["heads"][0]["revision"],"expected_mapping_revision":m["revision"],"mapping":{"executable":exe,"args":args,"env_refs":{"API_TOKEN":"env:FIXTURE_ONLY"}}})).unwrap();
    assert!(a
        .call("mcp.prepare", json!({"id":c["id"],"target_agent":"codex"}))
        .is_err());
    assert_eq!(m["scope"], "device");
}
