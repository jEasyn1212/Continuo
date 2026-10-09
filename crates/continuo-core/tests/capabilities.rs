use continuo_core::{
    api::{Policy, Service},
    crypto,
    sync::{configure, configured, SyncBackend},
};
use serde_json::{json, Value};
use std::process::Command;
fn create(s: &Service, name: &str, data: Value) -> Value {
    s.call(
        "entity.create",
        json!({"kind":"capability","name":name,"data":data}),
    )
    .unwrap()
}
fn review(s: &Service, e: &Value) -> Value {
    let i = s.call("capability.inspect", json!({"id":e["id"]})).unwrap();
    s.call("capability.review",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_digest":i["digest"]})).unwrap()
}
fn prepare(s: &Service, e: &Value, agent: &str) -> continuo_core::Result<Value> {
    s.call(
        "agent.prepare",
        json!({"agent":agent,"cwd":"/tmp","use_current_identity":false,"capability_ids":[e["id"]]}),
    )
}
#[test]
fn imported_text_requires_exact_review_and_adapters_apply_dependency_first_without_permissions() {
    let t = tempfile::tempdir().unwrap();
    let s = Service::open(&t.path().join("vault"), Policy::local_user()).unwrap();
    let dep = review(
        &s,
        &create(&s, "Sources", json!({"body":"Check primary evidence"})),
    );
    let literal = "Literal $(do-not-run) `commands` <script>never execute</script>";
    let cap=s.call("capability.import_text",json!({"name":"Research skill","body":literal,"capability_type":"skill","version":"1.2","source_ref":"project:skills/research/SKILL.md","source_revision":"fixture-commit","source_license":"MIT"})).unwrap();
    assert_eq!(
        prepare(&s, &cap, "codex").unwrap_err().code,
        "capability_not_ready"
    );
    let inspect = s
        .call("capability.inspect", json!({"id":cap["id"]}))
        .unwrap();
    assert_eq!(inspect["permissions_granted"], false);
    assert_eq!(inspect["source_verified"], false);
    assert_eq!(s.call("capability.review",json!({"id":cap["id"],"expected_revision":cap["heads"][0]["revision"],"expected_digest":"wrong"})).unwrap_err().code,"revision_conflict");
    let mut data = cap["heads"][0]["data"].clone();
    data["requires"] = json!([dep["id"]]);
    data["extension"] = json!({"retained":true});
    let cap = s
        .call(
            "entity.update",
            json!({"id":cap["id"],"expected_revision":cap["heads"][0]["revision"],"data":data}),
        )
        .unwrap();
    let cap = review(&s, &cap);
    let id=s.call("entity.create",json!({"kind":"identity","name":"Research","data":{"instructions":"Be precise","capability_ids":[cap["id"]]}})).unwrap();
    let task=s.call("entity.create",json!({"kind":"task","name":"Research","data":{"goal":"Verify sources","next_steps":["Read evidence"],"identity_id":id["id"]}})).unwrap();
    for agent in ["claude-code", "codex", "hermes"] {
        let plan=s.call("agent.prepare",json!({"agent":agent,"cwd":"/tmp","task_id":task["id"],"expected_task_revision":task["heads"][0]["revision"],"capability_ids":[cap["id"],dep["id"]]})).unwrap();
        let c = &plan["capability_context"];
        assert_eq!(c["applications"].as_array().unwrap().len(), 2);
        assert_eq!(c["applications"][0]["id"], dep["id"]);
        assert_eq!(
            c["applications"][1]["revision"],
            cap["heads"][0]["revision"]
        );
        assert_eq!(c["scripts_executed"], false);
        assert_eq!(c["permissions_granted"], false);
        assert_eq!(c["config_written"], false);
        assert_eq!(plan["executed"], false);
        let args = plan["args"].as_array().unwrap();
        let text = if agent == "codex" {
            serde_json::from_str::<String>(
                args.iter()
                    .find_map(|a| a.as_str().unwrap().strip_prefix("developer_instructions="))
                    .unwrap(),
            )
            .unwrap()
        } else {
            args.iter()
                .filter_map(Value::as_str)
                .find(|a| a.contains(literal))
                .unwrap()
                .into()
        };
        assert!(text.contains(literal));
        assert!(text.find("Check primary evidence").unwrap() < text.find(literal).unwrap());
    }
    let old = s
        .call("capability.inspect", json!({"id":cap["id"]}))
        .unwrap();
    let mut data = cap["heads"][0]["data"].clone();
    data["body"] = json!("Updated content");
    let changed = s
        .call(
            "entity.update",
            json!({"id":cap["id"],"expected_revision":cap["heads"][0]["revision"],"data":data}),
        )
        .unwrap();
    assert_eq!(changed["heads"][0]["data"]["extension"]["retained"], true);
    assert_eq!(
        prepare(&s, &changed, "codex").unwrap_err().code,
        "capability_not_ready"
    );
    assert_eq!(s.call("capability.review",json!({"id":cap["id"],"expected_revision":old["capability"]["revision"],"expected_digest":old["digest"]})).unwrap_err().code,"revision_conflict");
    let readonly = Service::open(&t.path().join("vault"), Policy::read_only()).unwrap();
    assert_eq!(
        readonly
            .call("capability.import_text", json!({"name":"bad","body":"x"}))
            .unwrap_err()
            .code,
        "permission_denied"
    );
}
#[test]
fn incompatible_deleted_missing_and_cyclic_dependencies_fail_closed_but_remain_repairable() {
    let t = tempfile::tempdir().unwrap();
    let s = Service::open(&t.path().join("vault"), Policy::local_user()).unwrap();
    let a = review(
        &s,
        &create(&s, "A", json!({"body":"A","agent_targets":["codex"]})),
    );
    assert!(prepare(&s, &a, "codex").is_ok());
    let err = prepare(&s, &a, "hermes").unwrap_err();
    assert_eq!(
        err.details.unwrap()["issues"][0]["code"],
        "agent_incompatible"
    );
    let b = review(
        &s,
        &create(&s, "B", json!({"body":"B","requires":[a["id"]]})),
    );
    let mut data = a["heads"][0]["data"].clone();
    data["requires"] = json!([b["id"]]);
    assert_eq!(
        s.call(
            "entity.update",
            json!({"id":a["id"],"expected_revision":a["heads"][0]["revision"],"data":data})
        )
        .unwrap_err()
        .code,
        "capability_dependency_cycle"
    );
    let mut incoming = s.store.get(a["id"].as_str().unwrap()).unwrap().heads[0].clone();
    incoming.parents = vec![incoming.revision.clone()];
    incoming.revision = uuid::Uuid::new_v4().to_string();
    incoming.data = data;
    s.store.import_events(&[incoming.clone()]).unwrap();
    assert_eq!(
        prepare(&s, &b, "codex").unwrap_err().code,
        "capability_not_ready"
    );
    assert!(
        s.call("capability.inspect", json!({"id":b["id"]})).unwrap()["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "dependency_cycle")
    );
    incoming.data["requires"] = json!([uuid::Uuid::new_v4().to_string()]);
    incoming.parents = vec![incoming.revision.clone()];
    incoming.revision = uuid::Uuid::new_v4().to_string();
    s.store.import_events(&[incoming.clone()]).unwrap();
    let inspected = s.call("capability.inspect", json!({"id":a["id"]})).unwrap();
    assert!(inspected["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == "not_found"));
    let mut repair = incoming.data.clone();
    repair["requires"] = json!([]);
    let a = s
        .call(
            "entity.update",
            json!({"id":a["id"],"expected_revision":incoming.revision,"data":repair}),
        )
        .unwrap();
    let a = review(&s, &a);
    assert!(prepare(&s, &b, "codex").is_ok());
    s.call(
        "entity.delete",
        json!({"id":a["id"],"expected_revision":a["heads"][0]["revision"]}),
    )
    .unwrap();
    assert!(prepare(&s, &b, "codex").is_err());
    for bad in [
        json!({"body":42}),
        json!({"body":"x","source_ref":"/Users/private/file"}),
        json!({"body":"x","source_ref":"https://user:secret@host/x"}),
        json!({"body":"x","agent_targets":["new-agent"]}),
        json!({"body":"x","requires":[uuid::Uuid::new_v4().to_string()]}),
    ] {
        assert!(s
            .call(
                "entity.create",
                json!({"kind":"capability","name":"bad","data":bad})
            )
            .is_err());
    }
}
#[test]
fn encrypted_git_preserves_offline_content_and_tombstone_conflicts_until_explicit_resolution() {
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
    let original = review(&a, &create(&a, "Rule", json!({"body":"Base"})));
    configured(&a.store).unwrap().run(&a.store).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    a.call("entity.update",json!({"id":original["id"],"expected_revision":original["heads"][0]["revision"],"data":{"body":"Offline A"}})).unwrap();
    b.call(
        "entity.delete",
        json!({"id":original["id"],"expected_revision":original["heads"][0]["revision"]}),
    )
    .unwrap();
    configured(&a.store).unwrap().run(&a.store).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    configured(&a.store).unwrap().run(&a.store).unwrap();
    let conflict = a.store.get(original["id"].as_str().unwrap()).unwrap();
    assert!(conflict.conflicted);
    assert!(conflict.heads.iter().any(|h| h.deleted));
    assert!(prepare(&a, &original, "codex").is_err());
    let heads = conflict
        .heads
        .iter()
        .map(|h| &h.revision)
        .collect::<Vec<_>>();
    let merged=a.call("entity.resolve",json!({"id":original["id"],"expected_heads":heads,"name":"Rule merged","data":{"body":"Reviewed merged content","version":"2"}})).unwrap();
    let merged = review(&a, &merged);
    configured(&a.store).unwrap().run(&a.store).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    assert!(prepare(&b, &merged, "codex").is_ok());
    assert_eq!(
        a.store.get(original["id"].as_str().unwrap()).unwrap().heads,
        b.store.get(original["id"].as_str().unwrap()).unwrap().heads
    );
}
