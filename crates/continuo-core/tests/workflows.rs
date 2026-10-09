use continuo_core::{
    api::{Policy, Service},
    crypto,
    store::Store,
    sync::{configure, configured, SyncBackend},
};
use serde_json::json;
use std::process::Command;

#[test]
fn adapter_registry_drives_api_schemas_without_storage_changes() {
    use continuo_core::adapters::{AgentAdapter, Descriptor, LaunchPlan, LaunchRequest, Registry};
    struct Fixture;
    impl AgentAdapter for Fixture {
        fn descriptor(&self) -> Descriptor {
            Descriptor {
                id: "fixture",
                name: "Contract fixture",
                executable: "fixture",
                capabilities: json!({"process_execution":false}),
            }
        }
        fn prepare_launch(&self, r: &LaunchRequest) -> continuo_core::Result<LaunchPlan> {
            Ok(LaunchPlan {
                agent: "fixture".into(),
                executable: "fixture".into(),
                args: vec![],
                cwd: r.cwd.clone(),
                env: Default::default(),
                executed: false,
                warnings: vec![],
            })
        }
        fn mcp_registration(&self, _: &str, _: &[String]) -> serde_json::Value {
            json!({})
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut service = Service::open(&dir.path().join("vault"), Policy::local_user()).unwrap();
    service.registry = Registry::new(vec![Box::new(Fixture)]).unwrap();
    let catalog = service.available();
    let prepare = catalog
        .iter()
        .find(|o| o.method == "agent.prepare")
        .unwrap();
    assert_eq!(
        prepare.input_schema["properties"]["agent"]["enum"],
        json!(["fixture"])
    );
    assert_eq!(
        service
            .call("agent.prepare", json!({"agent":"fixture","cwd":"/tmp"}))
            .unwrap()["agent"],
        "fixture"
    );
    assert!(service
        .call("agent.prepare", json!({"agent":"codex","cwd":"/tmp"}))
        .is_err());
    assert!(Registry::new(vec![Box::new(Fixture), Box::new(Fixture)]).is_err());
    // A new adapter must explicitly implement capability application; never silently drop text.
    let cap = service
        .store
        .create(
            "capability",
            "fixture content",
            json!({"body":"Literal fixture instructions"}),
        )
        .unwrap();
    let inspection = service
        .call("capability.inspect", json!({"id":cap.id}))
        .unwrap();
    let cap = service.call("capability.review", json!({"id":cap.id,"expected_revision":cap.heads[0].revision,"expected_digest":inspection["digest"]})).unwrap();
    assert_eq!(
        service
            .call(
                "agent.prepare",
                json!({"agent":"fixture","cwd":"/tmp","capability_ids":[cap["id"]]})
            )
            .unwrap_err()
            .code,
        "capability_unsupported"
    );
    // Historical session records remain portable even when this device lacks their adapter.
    let record = service
        .store
        .create("session", "portable", json!({"agent":"codex"}))
        .unwrap();
    assert_eq!(
        service.store.get(&record.id).unwrap().heads[0].data["agent"],
        "codex"
    );
}

fn bare(path: &std::path::Path) {
    assert!(Command::new("git")
        .args(["init", "--bare", "--quiet"])
        .arg(path)
        .status()
        .unwrap()
        .success());
}
fn setup_sync(store: &Store, remote: &std::path::Path, key: &std::path::Path) {
    configure(store, remote.to_str().unwrap(), key.to_str().unwrap()).unwrap();
}

#[test]
fn offline_concurrent_edits_converge_and_resolution_propagates() {
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remote.git");
    bare(&remote);
    let key = root.path().join("sync.key");
    crypto::generate_key(&key).unwrap();
    let a = Store::open(&root.path().join("device-a")).unwrap();
    let b = Store::open(&root.path().join("device-b")).unwrap();
    setup_sync(&a, &remote, &key);
    setup_sync(&b, &remote, &key);
    let original = a
        .create(
            "identity",
            "work",
            json!({"instructions":"company-private-text"}),
        )
        .unwrap();
    configured(&a).unwrap().run(&a).unwrap();
    configured(&b).unwrap().run(&b).unwrap();
    let id = &original.id;
    let revision = &original.heads[0].revision;
    a.update(
        id,
        revision,
        None,
        Some(json!({"instructions":"edit on A"})),
        false,
    )
    .unwrap();
    b.update(
        id,
        revision,
        None,
        Some(json!({"instructions":"edit on B"})),
        false,
    )
    .unwrap();
    configured(&a).unwrap().run(&a).unwrap();
    configured(&b).unwrap().run(&b).unwrap();
    configured(&a).unwrap().run(&a).unwrap();
    let conflict = a.get(id).unwrap();
    assert!(conflict.conflicted);
    assert_eq!(conflict.heads.len(), 2);
    assert_eq!(a.get(id).unwrap().heads, b.get(id).unwrap().heads);
    let heads = conflict
        .heads
        .iter()
        .map(|e| e.revision.clone())
        .collect::<Vec<_>>();
    a.resolve(
        id,
        &heads,
        "work",
        json!({"instructions":"merged A and B"}),
        false,
    )
    .unwrap();
    configured(&a).unwrap().run(&a).unwrap();
    configured(&b).unwrap().run(&b).unwrap();
    assert!(!b.get(id).unwrap().conflicted);
    assert_eq!(
        b.get(id).unwrap().heads[0].data["instructions"],
        "merged A and B"
    );
    let before = b.events().unwrap().len();
    let again = configured(&b).unwrap().run(&b).unwrap();
    assert_eq!(again["uploaded"], 0);
    assert_eq!(again["downloaded"], 0);
    assert_eq!(again["published"], false);
    assert_eq!(b.events().unwrap().len(), before);
    let encrypted = Command::new("git")
        .arg("--git-dir")
        .arg(&remote)
        .args(["show", &format!("continuo-sync:events/{}.json", revision)])
        .output()
        .unwrap();
    assert!(!String::from_utf8(encrypted.stdout)
        .unwrap()
        .contains("company-private-text"));
}

#[test]
fn deleted_object_stays_deleted_after_old_device_reconnects() {
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("r.git");
    bare(&remote);
    let key = root.path().join("k");
    crypto::generate_key(&key).unwrap();
    let a = Store::open(&root.path().join("a")).unwrap();
    let b = Store::open(&root.path().join("b")).unwrap();
    setup_sync(&a, &remote, &key);
    setup_sync(&b, &remote, &key);
    let task = a
        .create("task", "finish", json!({"status":"active"}))
        .unwrap();
    configured(&a).unwrap().run(&a).unwrap();
    configured(&b).unwrap().run(&b).unwrap();
    a.update(&task.id, &task.heads[0].revision, None, None, true)
        .unwrap();
    configured(&a).unwrap().run(&a).unwrap();
    configured(&b).unwrap().run(&b).unwrap();
    assert!(b.list(None, false).unwrap().is_empty());
    assert!(b.get(&task.id).unwrap().heads[0].deleted);
    assert_eq!(
        b.update(&task.id, &task.heads[0].revision, None, None, false)
            .unwrap_err()
            .code,
        "revision_conflict"
    );
}

#[test]
fn wrong_key_and_invalid_graph_do_not_partially_import() {
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("r.git");
    bare(&remote);
    let key = root.path().join("k");
    let other = root.path().join("other");
    crypto::generate_key(&key).unwrap();
    crypto::generate_key(&other).unwrap();
    let a = Store::open(&root.path().join("a")).unwrap();
    let b = Store::open(&root.path().join("b")).unwrap();
    setup_sync(&a, &remote, &key);
    setup_sync(&b, &remote, &other);
    let item = a.create("task", "private", json!({})).unwrap();
    configured(&a).unwrap().run(&a).unwrap();
    assert!(configured(&b).unwrap().run(&b).is_err());
    assert!(b.events().unwrap().is_empty());
    let mut malformed = item.heads[0].clone();
    malformed.revision = uuid::Uuid::new_v4().to_string();
    malformed.parents = vec![uuid::Uuid::new_v4().to_string()];
    assert!(b
        .import_events(&[item.heads[0].clone(), malformed])
        .is_err());
    assert!(b.events().unwrap().is_empty());
}

#[test]
fn encryption_rejects_tampering_and_filename_substitution() {
    let root = tempfile::tempdir().unwrap();
    let s = Store::open(root.path()).unwrap();
    let e = s
        .create("task", "secret", json!({"description":"private content"}))
        .unwrap()
        .heads
        .remove(0);
    let key = [7; 32];
    let bytes = crypto::seal(&key, &e).unwrap();
    assert_eq!(crypto::open(&key, &e.revision, &bytes).unwrap(), e);
    assert!(crypto::open(&key, &uuid::Uuid::new_v4().to_string(), &bytes).is_err());
    let mut modified: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let mut ciphertext = modified["ciphertext"].as_str().unwrap().to_owned();
    ciphertext.replace_range(
        0..1,
        if ciphertext.starts_with('A') {
            "B"
        } else {
            "A"
        },
    );
    modified["ciphertext"] = json!(ciphertext);
    assert!(crypto::open(&key, &e.revision, &serde_json::to_vec(&modified).unwrap()).is_err());
}

#[test]
fn stale_writes_read_only_policy_and_credentials_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let s = Service::open(root.path(), Policy::local_user()).unwrap();
    let created = s
        .call(
            "entity.create",
            json!({"kind":"task","name":"test","data":{}}),
        )
        .unwrap();
    let p = json!({"id":created["id"],"expected_revision":created["heads"][0]["revision"],"name":"updated"});
    s.call("entity.update", p.clone()).unwrap();
    assert_eq!(
        s.call("entity.update", p).unwrap_err().code,
        "revision_conflict"
    );
    assert_eq!(
        s.call(
            "entity.create",
            json!({"kind":"mcp","name":"bad","data":{"env":{"API_KEY":"do not store"}}})
        )
        .unwrap_err()
        .code,
        "credential_not_allowed"
    );
    assert!(s.call("entity.list", json!({"surprise":true})).is_err());
    assert_eq!(
        s.call("sync.run", json!({})).unwrap_err().code,
        "permission_denied"
    );
    let readonly = Service::open(root.path(), Policy::read_only()).unwrap();
    assert_eq!(
        readonly
            .call("entity.create", json!({"kind":"task","name":"x","data":{}}))
            .unwrap_err()
            .code,
        "permission_denied"
    );
    assert_eq!(
        s.registry.list().iter().map(|a| a.id).collect::<Vec<_>>(),
        vec!["claude-code", "codex", "hermes"]
    );
    assert!(s.registry.get("opencode").is_err());
}

#[test]
fn adapter_plans_preserve_literal_prompts_and_never_disable_permissions() {
    let root = tempfile::tempdir().unwrap();
    let s = Service::open(root.path(), Policy::local_user()).unwrap();
    for agent in ["claude-code", "codex", "hermes"] {
        let prompt = "--dangerously-bypass-approvals-and-sandbox $(touch /tmp/no) `command`";
        let plan = s
            .call(
                "agent.prepare",
                json!({"agent":agent,"cwd":"/tmp/project","prompt":prompt}),
            )
            .unwrap();
        assert_eq!(plan["executed"], false);
        let args = plan["args"].as_array().unwrap();
        assert!(!args
            .iter()
            .any(|v| v == "--dangerously-bypass-approvals-and-sandbox"));
        assert!(args.iter().any(|v| v.as_str().unwrap().contains(prompt)));
    }
}

#[test]
fn concurrent_local_writers_get_a_revision_conflict_instead_of_lost_update() {
    let root = tempfile::tempdir().unwrap();
    let a = Store::open(root.path()).unwrap();
    let item = a.create("task", "shared", json!({})).unwrap();
    let path = root.path().to_path_buf();
    let id = item.id.clone();
    let rev = item.heads[0].revision.clone();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2)
        .map(|n| {
            let (path, id, rev, barrier) = (path.clone(), id.clone(), rev.clone(), barrier.clone());
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap();
                barrier.wait();
                store.update(&id, &rev, None, Some(json!({"writer":n})), false)
            })
        })
        .collect::<Vec<_>>();
    let results = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .next()
            .unwrap()
            .code,
        "revision_conflict"
    );
}
