use continuo_core::{
    api::{Policy, Service},
    store::Store,
};
use serde_json::{json, Value};

fn create(s: &Service, kind: &str, name: &str, data: Value) -> Value {
    s.call(
        "entity.create",
        json!({"kind":kind,"name":name,"data":data}),
    )
    .unwrap()
}
fn activate(s: &Service, identity: &Value, expected: &str) -> Value {
    s.call("identity.activate", json!({"id":identity["id"],"expected_revision":identity["heads"][0]["revision"],"expected_selection_revision":expected})).unwrap()
}
#[test]
fn identity_profiles_bind_real_records_and_drive_all_adapter_plans() {
    let temp = tempfile::tempdir().unwrap();
    let s = Service::open(&temp.path().join("vault"), Policy::local_user()).unwrap();
    let capability = create(
        &s,
        "capability",
        "Research rules",
        json!({"description":"Check sources","body":"Use verified primary sources."}),
    );
    let inspected_cap = s
        .call("capability.inspect", json!({"id":capability["id"]}))
        .unwrap();
    s.call("capability.review", json!({"id":capability["id"],"expected_revision":capability["heads"][0]["revision"],"expected_digest":inspected_cap["digest"]})).unwrap();
    let mcp = create(
        &s,
        "mcp",
        "Local tools",
        json!({"credential_refs":["keychain:fixture"]}),
    );
    let instructions = "Literal 'quotes' $(do-not-run)\nUse precise sources.";
    let identity = create(
        &s,
        "identity",
        "Research",
        json!({"description":"Personal research", "instructions":instructions,"preferred_agent":"codex","capability_ids":[capability["id"]],"mcp_ids":[mcp["id"]],"extension":{"preserved":true}}),
    );
    let inspection = s
        .call("identity.inspect", json!({"id":identity["id"]}))
        .unwrap();
    assert_eq!(inspection["ready"], true);
    assert_eq!(inspection["bindings"].as_array().unwrap().len(), 2);
    assert_eq!(inspection["bindings_deployed"], false);
    assert_eq!(activate(&s, &identity, "none")["state"], "ready");
    for agent in ["claude-code", "codex", "hermes"] {
        let plan = s
            .call(
                "agent.prepare",
                json!({"agent":agent,"cwd":"/tmp","prompt":"Start"}),
            )
            .unwrap();
        assert_eq!(plan["executed"], false);
        assert_eq!(plan["identity_context"]["source"], "current");
        assert_eq!(
            plan["identity_context"]["identity"]["revision"],
            identity["heads"][0]["revision"]
        );
        let args = plan["args"].as_array().unwrap();
        if agent == "codex" {
            let encoded = args
                .iter()
                .filter_map(Value::as_str)
                .find_map(|a| a.strip_prefix("developer_instructions="))
                .unwrap();
            assert_eq!(
                serde_json::from_str::<String>(encoded)
                    .unwrap()
                    .contains(instructions),
                true
            );
        } else {
            assert!(args
                .iter()
                .any(|arg| arg.as_str().unwrap().contains(instructions)));
        }
        assert!(plan["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("not installed")));
    }
    let neutral = s
        .call(
            "agent.prepare",
            json!({"agent":"codex","cwd":"/tmp","use_current_identity":false}),
        )
        .unwrap();
    assert!(neutral["identity_context"].is_null());
    let explicit = s
        .call(
            "agent.prepare",
            json!({"agent":"codex","cwd":"/tmp","identity_id":identity["id"]}),
        )
        .unwrap();
    assert_eq!(explicit["identity_context"]["source"], "explicit");
    let mut profile = identity["heads"][0]["data"].clone();
    profile["instructions"] = json!("Updated instructions");
    let updated = s.call("entity.update", json!({"id":identity["id"],"expected_revision":identity["heads"][0]["revision"],"data":profile})).unwrap();
    assert_eq!(updated["heads"][0]["data"]["extension"]["preserved"], true);
    assert_eq!(
        s.call("identity.current", json!({})).unwrap()["context"]["profile"]["instructions"],
        "Updated instructions"
    );
    let remote = Store::open(&temp.path().join("other-device")).unwrap();
    remote.import_events(&s.store.events().unwrap()).unwrap();
    let other = Service::open(&temp.path().join("other-device"), Policy::local_user()).unwrap();
    assert_eq!(
        other.call("identity.current", json!({})).unwrap()["state"],
        "none"
    );
    assert_eq!(
        other
            .call("identity.inspect", json!({"id":identity["id"]}))
            .unwrap()["ready"],
        true
    );
}
#[test]
fn device_selection_cas_prevents_lost_switches_and_policy_is_enforced() {
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    let s = Service::open(&vault, Policy::local_user()).unwrap();
    let other = Service::open(&vault, Policy::local_user()).unwrap();
    let a = create(&s, "identity", "A", json!({"instructions":"A"}));
    let b = create(&s, "identity", "B", json!({"instructions":"B"}));
    let first = activate(&s, &a, "none");
    let err = other.call("identity.activate",json!({"id":b["id"],"expected_revision":b["heads"][0]["revision"],"expected_selection_revision":"none"})).unwrap_err();
    assert_eq!(err.code, "selection_conflict");
    let readonly = Service::open(&vault, Policy::read_only()).unwrap();
    assert_eq!(
        readonly.call("identity.current", json!({})).unwrap()["selection"]["identity_id"],
        a["id"]
    );
    assert_eq!(
        readonly
            .call(
                "identity.clear",
                json!({"expected_selection_revision":first["selection"]["revision"]})
            )
            .unwrap_err()
            .code,
        "permission_denied"
    );
    s.call(
        "entity.update",
        json!({"id":b["id"],"expected_revision":b["heads"][0]["revision"],"name":"New B"}),
    )
    .unwrap();
    assert_eq!(s.call("identity.activate",json!({"id":b["id"],"expected_revision":b["heads"][0]["revision"],"expected_selection_revision":first["selection"]["revision"]})).unwrap_err().code,"revision_conflict");
    assert_eq!(
        other
            .call(
                "identity.clear",
                json!({"expected_selection_revision":first["selection"]["revision"]})
            )
            .unwrap()["state"],
        "none"
    );
}
#[test]
fn invalid_bindings_cannot_bypass_domain_validation_and_broken_current_never_silently_launches() {
    let temp = tempfile::tempdir().unwrap();
    let s = Service::open(&temp.path().join("vault"), Policy::local_user()).unwrap();
    let task = create(&s, "task", "wrong kind", json!({}));
    let cap = create(&s, "capability", "capability", json!({}));
    let missing = uuid::Uuid::new_v4().to_string();
    for ids in [
        json!([task["id"]]),
        json!([missing]),
        json!([cap["id"], cap["id"]]),
    ] {
        assert!(s
            .call(
                "entity.create",
                json!({"kind":"identity","name":"invalid","data":{"capability_ids":ids}})
            )
            .is_err());
    }
    assert_eq!(s.store.list(Some("identity"), false).unwrap().len(), 0);
    assert!(s
        .call(
            "entity.create",
            json!({"kind":"identity","name":"bad types","data":{"instructions":42}})
        )
        .is_err());
    let id = create(
        &s,
        "identity",
        "valid",
        json!({"capability_ids":[cap["id"]]}),
    );
    activate(&s, &id, "none");
    s.call(
        "entity.delete",
        json!({"id":cap["id"],"expected_revision":cap["heads"][0]["revision"]}),
    )
    .unwrap();
    assert_eq!(
        s.call("identity.current", json!({})).unwrap()["state"],
        "unavailable"
    );
    assert_eq!(
        s.call("agent.prepare", json!({"agent":"codex","cwd":"/tmp"}))
            .unwrap_err()
            .code,
        "identity_bindings_unavailable"
    );
    assert!(s.call("entity.update",json!({"id":id["id"],"expected_revision":id["heads"][0]["revision"],"name":"still invalid"})).is_err());
    let repaired = s.call("entity.update",json!({"id":id["id"],"expected_revision":id["heads"][0]["revision"],"data":{"capability_ids":[]}})).unwrap();
    assert_eq!(
        s.call("identity.current", json!({})).unwrap()["state"],
        "ready"
    );
    s.call(
        "entity.delete",
        json!({"id":id["id"],"expected_revision":repaired["heads"][0]["revision"]}),
    )
    .unwrap();
    assert_eq!(
        s.call("identity.current", json!({})).unwrap()["state"],
        "unavailable"
    );
    assert_eq!(
        s.call("agent.prepare", json!({"agent":"codex","cwd":"/tmp"}))
            .unwrap_err()
            .code,
        "invalid_identity"
    );
}
#[test]
fn synchronized_conflicts_and_missing_dependencies_remain_visible_and_can_be_repaired() {
    let temp = tempfile::tempdir().unwrap();
    let a = Service::open(&temp.path().join("a"), Policy::local_user()).unwrap();
    let b = Service::open(&temp.path().join("b"), Policy::local_user()).unwrap();
    let id = create(&a, "identity", "Research", json!({"instructions":"base"}));
    b.store.import_events(&a.store.events().unwrap()).unwrap();
    activate(&a, &id, "none");
    for (service, instructions) in [(&a, "branch-a"), (&b, "branch-b")] {
        service.call("entity.update",json!({"id":id["id"],"expected_revision":id["heads"][0]["revision"],"data":{"instructions":instructions}})).unwrap();
    }
    a.store.import_events(&b.store.events().unwrap()).unwrap();
    assert_eq!(
        a.call("identity.current", json!({})).unwrap()["state"],
        "unavailable"
    );
    assert!(a
        .call("agent.prepare", json!({"agent":"hermes","cwd":"/tmp"}))
        .is_err());
    let conflict = a.store.get(id["id"].as_str().unwrap()).unwrap();
    let merged = a.call("entity.resolve",json!({"id":id["id"],"expected_heads":conflict.heads.iter().map(|h| &h.revision).collect::<Vec<_>>(),"name":"merged","data":{"instructions":"both"}})).unwrap();
    assert_eq!(
        a.call("identity.current", json!({})).unwrap()["state"],
        "ready"
    );
    let mut incoming = a.store.get(id["id"].as_str().unwrap()).unwrap().heads[0].clone();
    incoming.parents = vec![merged["heads"][0]["revision"].as_str().unwrap().into()];
    incoming.revision = uuid::Uuid::new_v4().to_string();
    incoming.data["mcp_ids"] = json!([uuid::Uuid::new_v4().to_string()]);
    a.store.import_events(&[incoming]).unwrap();
    assert_eq!(
        a.call("identity.inspect", json!({"id":id["id"]})).unwrap()["bindings"][0]["state"],
        "missing"
    );
    assert_eq!(
        a.call("identity.current", json!({})).unwrap()["state"],
        "unavailable"
    );
}
