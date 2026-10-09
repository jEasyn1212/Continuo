use continuo_core::{
    api::{Policy, Service},
    crypto,
    sync::{configure, configured, SyncBackend},
};
use serde_json::{json, Value};
use std::path::Path;
fn setup(s: &Service, agent: &str) -> (Value, Value, Value) {
    let i=s.call("entity.create",json!({"kind":"identity","name":"Review","data":{"instructions":"User recorded identity guidance"}})).unwrap();
    let t=s.call("entity.create",json!({"kind":"task","name":"Review task","data":{"goal":"Review a change","next_steps":["Read source"],"identity_id":i["id"]}})).unwrap();
    let e=s.call("session.register",json!({"name":"Review session","data":{"agent":agent,"native_session_id":"fixture-native-1","task_id":t["id"],"identity_id":i["id"],"environment_ref":"project:review","summary":"User supplied progress","extension":{"kept":true}}})).unwrap();
    (i, t, e)
}
fn map(s: &Service, e: &Value, root: &Path, expected: &str, confirmed: bool) -> Value {
    let exe = root.join("isolated-runtime");
    std::fs::write(&exe, "#!/bin/sh\nexit 99\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    s.call("session.map",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":expected,"mapping":{"executable":exe,"cwd":root,"runtime_version":"user entered fixture version","account_ref":"credential:fixture-account","state_present_confirmed":confirmed}})).unwrap()["mapping"].clone()
}
#[test]
fn native_resume_and_cross_agent_context_are_distinct_for_all_adapters_without_execution_or_history_reads(
) {
    let tmp = tempfile::tempdir().unwrap();
    let s = Service::open(&tmp.path().join("vault"), Policy::local_user()).unwrap();
    for agent in ["claude-code", "codex", "hermes"] {
        let (_, t, e) = setup(&s, agent);
        let before = s.call("session.inspect", json!({"id":e["id"]})).unwrap();
        assert_eq!(before["resume"]["ready"], false);
        assert_eq!(before["continuation"]["ready"], true);
        assert_eq!(before["history_read"], false);
        assert_eq!(
            before["profile"]["origin_device_id"],
            s.store.device_id().unwrap()
        );
        let m = map(&s, &e, tmp.path(), "none", true);
        let p=s.call("session.resume_plan",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":m["revision"]})).unwrap();
        assert_eq!(p["mode"], "native_resume");
        assert_eq!(p["executed"], false);
        assert_eq!(p["identity_instructions_reapplied"], false);
        assert_eq!(p["account_authentication_verified"], false);
        let args = p["args"].as_array().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        let expected = match agent {
            "claude-code" => json!(["--resume=fixture-native-1"]),
            "codex" => json!(["resume", "-C", cwd, "--", "fixture-native-1"]),
            "hermes" => json!(["--in", cwd, "--resume=fixture-native-1", "chat"]),
            _ => unreachable!(),
        };
        assert_eq!(p["args"], expected);
        assert!(args
            .iter()
            .any(|a| a.as_str().is_some_and(|a| a.contains("fixture-native-1"))));
        assert!(!p.to_string().contains("User recorded identity guidance"));
        let packet=s.call("session.packet",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"target_agent":"codex","expected_task_revision":t["heads"][0]["revision"]})).unwrap();
        assert!(packet["prompt"]
            .as_str()
            .unwrap()
            .contains("User supplied progress"));
        assert_eq!(packet["native_session_id_transferred"], false);
        assert!(!packet.to_string().contains("fixture-native-1"));
        let continued=s.call("session.continue_plan",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"target_agent":"codex","expected_task_revision":t["heads"][0]["revision"],"cwd":tmp.path()})).unwrap();
        assert_eq!(continued["mode"], "context_handoff");
        assert_eq!(continued["executed"], false);
        assert!(continued
            .to_string()
            .contains("User recorded identity guidance"));
        assert!(!continued["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "resume"));
        assert!(!continued.to_string().contains("fixture-native-1"));
        assert!(s.call("session.packet",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"target_agent":"codex","expected_task_revision":uuid::Uuid::new_v4().to_string()})).is_err());
    }
}
#[test]
fn recorded_cancel_and_reopen_require_reasons_and_broken_links_remain_repairable() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Service::open(&tmp.path().join("vault"), Policy::local_user()).unwrap();
    let (i, t, e) = setup(&s, "codex");
    let denied = Service::open(&tmp.path().join("vault"), Policy::read_only()).unwrap();
    assert_eq!(denied.call("session.transition",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"status":"cancelled","reason":"Stop recorded work"})).unwrap_err().code,"permission_denied");
    assert_eq!(s.call("entity.update",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"data":{"agent":"codex","status":"active"}})).unwrap_err().code,"session_transition_requires_reason");
    let cancelled=s.call("session.transition",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"status":"cancelled","reason":"User stops this record"})).unwrap();
    assert_eq!(cancelled["heads"][0]["data"]["extension"]["kept"], true);
    let inspect = s.call("session.inspect", json!({"id":e["id"]})).unwrap();
    assert_eq!(inspect["continuation"]["ready"], false);
    assert_eq!(
        inspect["status_verification"],
        "user_recorded_not_process_observation"
    );
    assert!(s.call("session.transition",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"status":"paused","reason":"stale"})).is_err());
    let reopened=s.call("session.transition",json!({"id":e["id"],"expected_revision":cancelled["heads"][0]["revision"],"status":"paused","reason":"Reopen explicitly"})).unwrap();
    let other = s
        .call(
            "entity.create",
            json!({"kind":"identity","name":"Other","data":{}}),
        )
        .unwrap();
    let mut d = reopened["heads"][0]["data"].clone();
    d["identity_id"] = other["id"].clone();
    assert_eq!(
        s.call(
            "entity.update",
            json!({"id":e["id"],"expected_revision":reopened["heads"][0]["revision"],"data":d})
        )
        .unwrap_err()
        .code,
        "session_identity_mismatch"
    );
    s.call(
        "entity.delete",
        json!({"id":i["id"],"expected_revision":i["heads"][0]["revision"]}),
    )
    .unwrap();
    assert_eq!(
        s.call("session.inspect", json!({"id":e["id"]})).unwrap()["continuation"]["ready"],
        false
    );
    assert!(s.call("session.packet",json!({"id":e["id"],"expected_revision":reopened["heads"][0]["revision"],"target_agent":"hermes","expected_task_revision":t["heads"][0]["revision"]})).is_err());
    let mut d = reopened["heads"][0]["data"].clone();
    d["task_id"] = Value::Null;
    d["identity_id"] = Value::Null;
    s.call(
        "entity.update",
        json!({"id":e["id"],"expected_revision":reopened["heads"][0]["revision"],"data":d}),
    )
    .unwrap();
}
#[test]
fn local_environment_confirmation_and_revision_tombstones_prevent_stale_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Service::open(&tmp.path().join("vault"), Policy::local_user()).unwrap();
    let (_, _, e) = setup(&s, "hermes");
    let m = map(&s, &e, tmp.path(), "none", false);
    assert_eq!(
        s.call("session.inspect", json!({"id":e["id"]})).unwrap()["resume"]["ready"],
        false
    );
    assert!(s.call("session.resume_plan",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":m["revision"]})).is_err());
    let m = map(&s, &e, tmp.path(), m["revision"].as_str().unwrap(), true);
    let mut d = e["heads"][0]["data"].clone();
    d["native_session_id"] = json!("fixture-new-id");
    let e = s
        .call(
            "entity.update",
            json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"data":d}),
        )
        .unwrap();
    assert!(
        s.call("session.inspect", json!({"id":e["id"]})).unwrap()["resume"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["code"] == "session_mapping_stale")
    );
    let c = s
        .call(
            "session.clear_mapping",
            json!({"id":e["id"],"expected_mapping_revision":m["revision"]}),
        )
        .unwrap();
    assert_eq!(s.call("session.map",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":"none","mapping":{"executable":tmp.path().join("isolated-runtime"),"cwd":tmp.path()}})).unwrap_err().code,"session_mapping_conflict");
    let m = map(
        &s,
        &e,
        tmp.path(),
        c["mapping_revision"].as_str().unwrap(),
        true,
    );
    std::fs::remove_file(tmp.path().join("isolated-runtime")).unwrap();
    assert!(s.call("session.resume_plan",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":m["revision"]})).is_err());
    for data in [
        json!({"agent":"codex","native_session_id":"--danger"}),
        json!({"cwd":"/private/path"}),
        json!({"messages":["private history"]}),
        json!({"environment_ref":"/private/workspace"}),
    ] {
        assert!(s
            .call("session.register", json!({"name":"Invalid","data":data}))
            .is_err());
    }
}
#[test]
fn encrypted_sync_preserves_session_conflicts_and_references_without_local_environment_or_native_state(
) {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    assert!(std::process::Command::new("git")
        .args(["init", "--bare", "--quiet"])
        .arg(&remote)
        .status()
        .unwrap()
        .success());
    let key = tmp.path().join("key");
    crypto::generate_key(&key).unwrap();
    let a = Service::open(&tmp.path().join("a"), Policy::local_user()).unwrap();
    let b = Service::open(&tmp.path().join("b"), Policy::local_user()).unwrap();
    for s in [&a, &b] {
        configure(&s.store, remote.to_str().unwrap(), key.to_str().unwrap()).unwrap();
    }
    let (_, _, e) = setup(&a, "claude-code");
    map(&a, &e, tmp.path(), "none", true);
    configured(&a.store).unwrap().run(&a.store).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    let i = b.call("session.inspect", json!({"id":e["id"]})).unwrap();
    assert!(i["mapping"].is_null());
    assert_eq!(i["resume"]["ready"], false);
    assert_eq!(i["continuation"]["ready"], true);
    assert_eq!(
        i["profile"]["origin_device_id"],
        a.store.device_id().unwrap()
    );
    let events = serde_json::to_string(&a.store.events().unwrap()).unwrap();
    assert!(!events.contains(tmp.path().to_str().unwrap()));
    assert!(!events.contains("fixture-account"));
    for (s, summary) in [(&a, "A"), (&b, "B")] {
        let mut d = e["heads"][0]["data"].clone();
        d["summary"] = json!(summary);
        s.call(
            "entity.update",
            json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"data":d}),
        )
        .unwrap();
    }
    configured(&a.store).unwrap().run(&a.store).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    assert_eq!(
        b.call("session.inspect", json!({"id":e["id"]}))
            .unwrap_err()
            .code,
        "session_unavailable"
    );
    let heads = b.store.get(e["id"].as_str().unwrap()).unwrap().heads;
    let mut d = e["heads"][0]["data"].clone();
    d["summary"] = json!("Explicit merged summary");
    b.call("entity.resolve",json!({"id":e["id"],"expected_heads":heads.iter().map(|h|h.revision.clone()).collect::<Vec<_>>(),"name":"Merged session","data":d})).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    configured(&a.store).unwrap().run(&a.store).unwrap();
    assert_eq!(
        a.call("session.inspect", json!({"id":e["id"]})).unwrap()["profile"]["summary"],
        "Explicit merged summary"
    );
}

#[test]
fn unimplemented_resume_contract_fails_explicitly_without_changing_the_product_registry() {
    use continuo_core::adapters::{AgentAdapter, Descriptor, LaunchPlan, LaunchRequest, Registry};
    struct MetadataOnly;
    impl AgentAdapter for MetadataOnly {
        fn descriptor(&self) -> Descriptor {
            Descriptor {
                id: "fixture-adapter",
                name: "Contract fixture",
                executable: "fixture",
                capabilities: json!({"native_resume_plan":true}),
            }
        }
        fn prepare_launch(&self, _: &LaunchRequest) -> continuo_core::Result<LaunchPlan> {
            panic!("Native resume must use the dedicated adapter contract");
        }
        fn mcp_registration(&self, _: &str, _: &[String]) -> Value {
            Value::Null
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Service::open(&tmp.path().join("vault"), Policy::local_user()).unwrap();
    assert_eq!(s.registry.list().len(), 3);
    s.registry = Registry::new(vec![Box::new(MetadataOnly)]).unwrap();
    assert_eq!(
        s.registry
            .get("fixture-adapter")
            .unwrap()
            .prepare_managed_config(&Value::Null)
            .unwrap_err()
            .code,
        "managed_config_unsupported"
    );
    let (_, _, e) = setup(&s, "fixture-adapter");
    let m = map(&s, &e, tmp.path(), "none", true);
    assert_eq!(s.call("session.resume_plan",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":m["revision"]})).unwrap_err().code,"session_adapter_unsupported");
}
