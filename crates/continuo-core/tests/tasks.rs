use continuo_core::{
    api::{Policy, Service},
    crypto,
    sync::{configure, configured, SyncBackend},
};
use serde_json::{json, Value};
use std::process::Command;
fn create(s: &Service) -> Value {
    s.call("entity.create",json!({"kind":"task","name":"Release review","data":{"goal":"Prepare an inspectable release","success_criteria":["Checks pass"],"next_steps":["Review change"],"extension":{"preserve":true}}})).unwrap()
}
fn record(s: &Service, task: &Value, method: &str, mut params: Value) -> Value {
    params["id"] = task["id"].clone();
    params["expected_revision"] = task["heads"][0]["revision"].clone();
    s.call(method, params).unwrap()
}
#[test]
fn task_workflow_records_reason_progress_decisions_and_artifacts_without_losing_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let s = Service::open(dir.path(), Policy::local_user()).unwrap();
    let mut task = create(&s);
    task = record(
        &s,
        &task,
        "task.transition",
        json!({"status":"active","reason":"Start review"}),
    );
    task = record(
        &s,
        &task,
        "task.progress",
        json!({"summary":"Implemented checks","checks":["cargo test: passed"]}),
    );
    task = record(
        &s,
        &task,
        "task.decision",
        json!({"summary":"Use immutable records","reason":"Preserve both devices' changes"}),
    );
    task = record(
        &s,
        &task,
        "task.artifact",
        json!({"title":"Review notes","reference":"project:docs/review.md","verification":"Reviewed locally; receiver should verify"}),
    );
    assert_eq!(task["heads"][0]["data"]["extension"]["preserve"], true);
    assert_eq!(
        task["heads"][0]["data"]["progress"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(task["heads"][0]["data"]["progress"][0]["id"].is_string());
    let packet=s.call("task.handoff",json!({"task_id":task["id"],"target_agent":"codex","expected_revision":task["heads"][0]["revision"]})).unwrap();
    assert_eq!(
        packet["context"]["snapshot"]["artifact_verification"],
        "user_recorded_not_independently_verified"
    );
    assert_eq!(packet["checklist"].as_array().unwrap().len(), 5);
    assert!(packet["prompt"]
        .as_str()
        .unwrap()
        .contains("Preserve both devices' changes"));
    task = record(
        &s,
        &task,
        "task.transition",
        json!({"status":"blocked","reason":"Need review approval"}),
    );
    assert_eq!(
        task["heads"][0]["data"]["blockers"],
        json!(["Need review approval"])
    );
    assert!(s
        .call(
            "task.handoff",
            json!({"task_id":task["id"],"target_agent":"hermes"})
        )
        .is_ok());
    task = record(
        &s,
        &task,
        "task.transition",
        json!({"status":"active","reason":"Approval received"}),
    );
    assert_eq!(task["heads"][0]["data"]["blockers"], json!([]));
    task = record(
        &s,
        &task,
        "task.transition",
        json!({"status":"done","reason":"Reviewed all changes; checks passed"}),
    );
    assert_eq!(
        s.call(
            "task.handoff",
            json!({"task_id":task["id"],"target_agent":"codex"})
        )
        .unwrap_err()
        .code,
        "task_not_ready"
    );
    task = record(
        &s,
        &task,
        "task.transition",
        json!({"status":"active","reason":"Review new findings"}),
    );
    assert_eq!(
        s.call("task.inspect", json!({"id":task["id"]})).unwrap()["handoff_ready"],
        true
    );
}
#[test]
fn invalid_state_bypasses_private_references_stale_writes_and_readonly_mutations_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let s = Service::open(dir.path(), Policy::local_user()).unwrap();
    let task = create(&s);
    let mut data = task["heads"][0]["data"].clone();
    data["status"] = json!("done");
    data["completion_summary"] = json!("Attempt bypass");
    assert_eq!(
        s.call(
            "entity.update",
            json!({"id":task["id"],"expected_revision":task["heads"][0]["revision"],"data":data})
        )
        .unwrap_err()
        .code,
        "invalid_task_transition"
    );
    data["status"] = json!("active");
    assert_eq!(
        s.call(
            "entity.update",
            json!({"id":task["id"],"expected_revision":task["heads"][0]["revision"],"data":data})
        )
        .unwrap_err()
        .code,
        "task_transition_requires_reason"
    );
    for reference in [
        "/Users/private/result.md",
        "file:///tmp/private",
        "project:../escape",
        "project:/absolute",
        "https://user:secret@example.com/a",
        "https://example.com/a?token=secret",
        "https://example.com/a#secret",
    ] {
        assert!(s.call("task.artifact",json!({"id":task["id"],"expected_revision":task["heads"][0]["revision"],"title":"invalid","reference":reference})).is_err());
    }
    let updated = record(
        &s,
        &task,
        "task.progress",
        json!({"summary":"A","checks":[]}),
    );
    assert_eq!(s.call("task.progress",json!({"id":task["id"],"expected_revision":task["heads"][0]["revision"],"summary":"stale"})).unwrap_err().code,"revision_conflict");
    assert_eq!(s.call("task.handoff",json!({"task_id":task["id"],"target_agent":"codex","expected_revision":task["heads"][0]["revision"]})).unwrap_err().code,"revision_conflict");
    let readonly = Service::open(dir.path(), Policy::read_only()).unwrap();
    assert_eq!(readonly.call("task.progress",json!({"id":task["id"],"expected_revision":updated["heads"][0]["revision"],"summary":"denied"})).unwrap_err().code,"permission_denied");
    assert!(readonly
        .call(
            "task.handoff",
            json!({"task_id":task["id"],"target_agent":"codex"})
        )
        .is_ok());
    assert_eq!(s.store.events().unwrap().len(), 2);
}
#[test]
fn assigned_identity_is_snapshotted_and_missing_context_blocks_handoff_and_all_adapter_plans() {
    let dir = tempfile::tempdir().unwrap();
    let s = Service::open(dir.path(), Policy::local_user()).unwrap();
    let identity=s.call("entity.create",json!({"kind":"identity","name":"Research","data":{"instructions":"Use verified references"}})).unwrap();
    let other = s
        .call(
            "entity.create",
            json!({"kind":"identity","name":"Other","data":{}}),
        )
        .unwrap();
    let mut task = create(&s);
    let mut data = task["heads"][0]["data"].clone();
    data["identity_id"] = identity["id"].clone();
    task = s
        .call(
            "entity.update",
            json!({"id":task["id"],"expected_revision":task["heads"][0]["revision"],"data":data}),
        )
        .unwrap();
    for agent in ["claude-code", "codex", "hermes"] {
        let plan=s.call("agent.prepare",json!({"agent":agent,"cwd":"/tmp","task_id":task["id"],"expected_task_revision":task["heads"][0]["revision"]})).unwrap();
        assert_eq!(plan["executed"], false);
        assert_eq!(plan["identity_context"]["source"], "task");
        assert_eq!(
            plan["task_context"]["task_revision"],
            task["heads"][0]["revision"]
        );
        assert_eq!(
            plan["identity_context"]["identity"]["revision"],
            identity["heads"][0]["revision"]
        );
    }
    assert_eq!(
        s.call(
            "agent.prepare",
            json!({"agent":"codex","cwd":"/tmp","task_id":task["id"],"identity_id":other["id"]})
        )
        .unwrap_err()
        .code,
        "task_identity_mismatch"
    );
    s.call(
        "entity.delete",
        json!({"id":identity["id"],"expected_revision":identity["heads"][0]["revision"]}),
    )
    .unwrap();
    assert_eq!(
        s.call("task.inspect", json!({"id":task["id"]})).unwrap()["handoff_ready"],
        false
    );
    assert_eq!(
        s.call(
            "agent.prepare",
            json!({"agent":"codex","cwd":"/tmp","task_id":task["id"]})
        )
        .unwrap_err()
        .code,
        "task_not_ready"
    );
    let incomplete = s
        .call(
            "entity.create",
            json!({"kind":"task","name":"Needs planning","data":{}}),
        )
        .unwrap();
    let inspected = s
        .call("task.inspect", json!({"id":incomplete["id"]}))
        .unwrap();
    assert_eq!(inspected["issues"].as_array().unwrap().len(), 2);
}
#[test]
fn offline_progress_versions_merge_over_encrypted_git_without_timestamp_winners() {
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remote.git");
    assert!(Command::new("git")
        .args(["init", "--bare", "--quiet"])
        .arg(&remote)
        .status()
        .unwrap()
        .success());
    let key = root.path().join("key");
    crypto::generate_key(&key).unwrap();
    let a = Service::open(&root.path().join("a"), Policy::local_user()).unwrap();
    let b = Service::open(&root.path().join("b"), Policy::local_user()).unwrap();
    for s in [&a, &b] {
        configure(&s.store, remote.to_str().unwrap(), key.to_str().unwrap()).unwrap();
    }
    let original = create(&a);
    configured(&a.store).unwrap().run(&a.store).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    record(
        &a,
        &original,
        "task.progress",
        json!({"summary":"Device A changed UI"}),
    );
    record(
        &b,
        &original,
        "task.progress",
        json!({"summary":"Device B changed core"}),
    );
    configured(&a.store).unwrap().run(&a.store).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    configured(&a.store).unwrap().run(&a.store).unwrap();
    let conflict = a.store.get(original["id"].as_str().unwrap()).unwrap();
    assert!(conflict.conflicted);
    assert!(a
        .call(
            "task.handoff",
            json!({"task_id":original["id"],"target_agent":"codex"})
        )
        .is_err());
    let mut merged = conflict.heads[0].data.clone();
    merged["progress"] = json!(conflict
        .heads
        .iter()
        .flat_map(|h| h.data["progress"].as_array().unwrap().clone())
        .collect::<Vec<_>>());
    a.call("entity.resolve",json!({"id":original["id"],"expected_heads":conflict.heads.iter().map(|h|&h.revision).collect::<Vec<_>>(),"name":"Merged review","data":merged})).unwrap();
    configured(&a.store).unwrap().run(&a.store).unwrap();
    configured(&b.store).unwrap().run(&b.store).unwrap();
    let inspected = b
        .call("task.inspect", json!({"id":original["id"]}))
        .unwrap();
    assert_eq!(
        inspected["profile"]["progress"].as_array().unwrap().len(),
        2
    );
    assert_eq!(inspected["handoff_ready"], true);
    assert_eq!(
        a.store.get(original["id"].as_str().unwrap()).unwrap().heads,
        b.store.get(original["id"].as_str().unwrap()).unwrap().heads
    );
}
