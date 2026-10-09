use continuo_core::{
    api::{Policy, Service},
    crypto,
    sync::{self, configure},
};
use serde_json::{json, Value};
use std::{path::Path, process::Command};
fn git(path: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(o.status.success(), "fixture git command failed");
    String::from_utf8(o.stdout).unwrap().trim().into()
}
fn fixture() -> (
    tempfile::TempDir,
    Service,
    Service,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let t = tempfile::tempdir().unwrap();
    let remote = t.path().join("remote.git");
    git(
        t.path(),
        &["init", "--bare", "--quiet", remote.to_str().unwrap()],
    );
    let key = t.path().join("key");
    crypto::generate_key(&key).unwrap();
    let policy = Policy {
        sync: true,
        ..Policy::local_user()
    };
    let a = Service::open(&t.path().join("a"), policy).unwrap();
    let b = Service::open(&t.path().join("b"), policy).unwrap();
    for s in [&a, &b] {
        configure(&s.store, remote.to_str().unwrap(), key.to_str().unwrap()).unwrap();
    }
    (t, a, b, remote, key)
}
fn task(s: &Service) -> Value {
    s.call("entity.create",json!({"kind":"task","name":"Portable work","data":{"goal":"Review changes","next_steps":["Check evidence"]}})).unwrap()
}
#[test]
fn preview_tracks_observed_pending_counts_without_importing_and_offline_retry_and_pause_preserve_local_events(
) {
    let (t, a, b, remote, _) = fixture();
    let e = task(&a);
    let p = a.call("sync.preview", json!({})).unwrap();
    assert_eq!(p["pending_upload"], 1);
    assert_eq!(p["published"], false);
    assert_eq!(a.store.events().unwrap().len(), 1);
    assert!(git(
        t.path(),
        &["--git-dir", remote.to_str().unwrap(), "branch", "--list"]
    )
    .is_empty());
    let r = a.call("sync.run", json!({})).unwrap();
    assert_eq!(r["uploaded"], 1);
    let id = r["run_id"].clone();
    assert_eq!(
        a.call("sync.run", json!({"run_id":id})).unwrap_err().code,
        "sync_job_exists"
    );
    let p = b.call("sync.preview", json!({})).unwrap();
    assert_eq!(p["pending_download"], 1);
    assert_eq!(b.store.events().unwrap().len(), 0);
    assert_eq!(
        b.call("sync.inspect", json!({})).unwrap()["pending_download"],
        1
    );
    b.call("sync.run", json!({})).unwrap();
    assert_eq!(b.store.events().unwrap().len(), 1);
    a.call(
        "entity.update",
        json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"name":"Offline change"}),
    )
    .unwrap();
    let moved = t.path().join("offline.git");
    std::fs::rename(&remote, &moved).unwrap();
    assert_eq!(
        a.call("sync.run", json!({})).unwrap_err().code,
        "git_failed"
    );
    assert_eq!(a.store.events().unwrap().len(), 2);
    assert_eq!(
        a.call("sync.inspect", json!({})).unwrap()["pending_upload"],
        1
    );
    std::fs::rename(&moved, &remote).unwrap();
    assert_eq!(a.call("sync.run", json!({})).unwrap()["uploaded"], 1);
    assert_eq!(a.call("sync.run", json!({})).unwrap()["uploaded"], 0);
    let i = a.call("sync.inspect", json!({})).unwrap();
    let paused = a
        .call(
            "sync.set_enabled",
            json!({"expected_config_revision":i["config_revision"],"enabled":false}),
        )
        .unwrap();
    assert_eq!(paused["device_revoked"], false);
    assert_eq!(
        a.call("sync.run", json!({})).unwrap_err().code,
        "sync_disabled"
    );
    assert_eq!(a.store.events().unwrap().len(), 2);
    assert_eq!(
        a.call(
            "sync.set_enabled",
            json!({"expected_config_revision":i["config_revision"],"enabled":true})
        )
        .unwrap_err()
        .code,
        "sync_config_changed"
    );
    a.call(
        "sync.set_enabled",
        json!({"expected_config_revision":paused["config_revision"],"enabled":true}),
    )
    .unwrap();
    assert_eq!(a.call("sync.run", json!({})).unwrap()["uploaded"], 0);
}
#[test]
fn missing_history_changed_or_lost_key_and_future_format_block_before_import_or_publish() {
    let (t, a, _, remote, key) = fixture();
    task(&a);
    a.call("sync.run", json!({})).unwrap();
    let tip = git(
        t.path(),
        &[
            "--git-dir",
            remote.to_str().unwrap(),
            "rev-parse",
            "refs/heads/continuo-sync",
        ],
    );
    git(
        t.path(),
        &[
            "--git-dir",
            remote.to_str().unwrap(),
            "update-ref",
            "-d",
            "refs/heads/continuo-sync",
        ],
    );
    assert_eq!(
        a.call("sync.run", json!({})).unwrap_err().code,
        "sync_remote_history_missing"
    );
    assert_eq!(a.store.events().unwrap().len(), 1);
    assert!(git(
        t.path(),
        &["--git-dir", remote.to_str().unwrap(), "branch", "--list"]
    )
    .is_empty());
    git(
        t.path(),
        &[
            "--git-dir",
            remote.to_str().unwrap(),
            "update-ref",
            "refs/heads/continuo-sync",
            &tip,
        ],
    );
    let original = std::fs::read(&key).unwrap();
    std::fs::write(&key, [77u8; 32]).unwrap();
    assert_eq!(
        a.call("sync.run", json!({})).unwrap_err().code,
        "sync_key_changed"
    );
    std::fs::write(&key, &original).unwrap();
    let backup = t.path().join("original-key");
    std::fs::rename(&key, &backup).unwrap();
    assert_eq!(
        a.call("sync.inspect", json!({})).unwrap()["key_status"]["code"],
        "sync_key_missing"
    );
    assert_eq!(
        a.call("sync.run", json!({})).unwrap_err().code,
        "sync_key_missing"
    );
    std::fs::rename(&backup, &key).unwrap();
    let copy = t.path().join("protocol-fixture");
    git(
        t.path(),
        &[
            "clone",
            "--quiet",
            "--branch",
            "continuo-sync",
            remote.to_str().unwrap(),
            copy.to_str().unwrap(),
        ],
    );
    let manifest = copy.join("manifest.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    value["version"] = json!(999);
    std::fs::write(&manifest, value.to_string()).unwrap();
    git(&copy, &["add", "manifest.json"]);
    git(&copy, &["commit", "--quiet", "-m", "Future format fixture"]);
    git(&copy, &["push", "--quiet"]);
    assert_eq!(
        a.call("sync.run", json!({})).unwrap_err().code,
        "incompatible_workspace"
    );
    assert_eq!(a.store.events().unwrap().len(), 1);
    assert_eq!(
        a.call("sync.inspect", json!({})).unwrap()["job"]["state"],
        "failed"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            a.call("sync.run", json!({})).unwrap_err().code,
            "unsafe_key_permissions"
        );
    }
}
#[test]
fn explicit_historical_restore_revalidates_links_and_propagates_new_event_without_erasing_tombstones(
) {
    let (_fixture, a, b, _, _) = fixture();
    let identity = a
        .call(
            "entity.create",
            json!({"kind":"identity","name":"Review","data":{}}),
        )
        .unwrap();
    let e=a.call("entity.create",json!({"kind":"task","name":"Work","data":{"status":"active","identity_id":identity["id"],"goal":"Review","next_steps":["Read"]}})).unwrap();
    let paused=a.call("task.transition",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"status":"paused","reason":"Wait"})).unwrap();
    let deleted = a
        .call(
            "entity.delete",
            json!({"id":e["id"],"expected_revision":paused["heads"][0]["revision"]}),
        )
        .unwrap();
    a.call("sync.run", json!({})).unwrap();
    b.call("sync.run", json!({})).unwrap();
    assert_eq!(
        b.store.get(e["id"].as_str().unwrap()).unwrap().heads[0].deleted,
        true
    );
    let h = a.call("entity.history", json!({"id":e["id"]})).unwrap();
    assert_eq!(h["total_versions"], 3);
    assert_eq!(h["timestamp_is_display_only"], true);
    assert!(a.call("entity.restore",json!({"id":e["id"],"expected_revision":deleted["heads"][0]["revision"],"source_revision":identity["heads"][0]["revision"]})).is_err());
    let identity_deleted = a
        .call(
            "entity.delete",
            json!({"id":identity["id"],"expected_revision":identity["heads"][0]["revision"]}),
        )
        .unwrap();
    assert_eq!(a.call("entity.restore",json!({"id":e["id"],"expected_revision":deleted["heads"][0]["revision"],"source_revision":e["heads"][0]["revision"]})).unwrap_err().code,"invalid_task_identity");
    a.call("entity.restore",json!({"id":identity["id"],"expected_revision":identity_deleted["heads"][0]["revision"],"source_revision":identity["heads"][0]["revision"]})).unwrap();
    let restored=a.call("entity.restore",json!({"id":e["id"],"expected_revision":deleted["heads"][0]["revision"],"source_revision":e["heads"][0]["revision"]})).unwrap();
    assert_eq!(restored["heads"][0]["data"]["status"], "active");
    assert_eq!(
        restored["heads"][0]["parents"],
        json!([deleted["heads"][0]["revision"]])
    );
    assert!(a.call("entity.restore",json!({"id":e["id"],"expected_revision":deleted["heads"][0]["revision"],"source_revision":e["heads"][0]["revision"]})).is_err());
    a.call("sync.run", json!({})).unwrap();
    b.call("sync.run", json!({})).unwrap();
    assert_eq!(
        b.store.get(e["id"].as_str().unwrap()).unwrap().heads[0].deleted,
        false
    );
    assert_eq!(
        a.call("entity.history", json!({"id":e["id"]})).unwrap()["total_versions"],
        4
    );
    assert!(!serde_json::to_string(&a.store.events().unwrap())
        .unwrap()
        .contains("sync_config"));
    assert_eq!(sync::inspect(&b.store).unwrap()["pending_upload"], 0);
}
