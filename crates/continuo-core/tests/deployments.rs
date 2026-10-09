use continuo_core::{
    adapters::Registry,
    api::{Policy, Service},
};
use serde_json::{json, Value};
use std::{fs, path::Path};
fn definition(s: &Service, name: &str) -> Value {
    let executable = s.store.data_dir.join("fixture-command");
    fs::write(&executable, "#!/bin/sh\nexit 3\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let e = s
        .call(
            "entity.create",
            json!({"kind":"mcp","name":name,"data":{"transport":"stdio","server_key":name}}),
        )
        .unwrap();
    s.call("mcp.map",json!({"id":e["id"],"expected_revision":e["heads"][0]["revision"],"expected_mapping_revision":"none","mapping":{"executable":executable,"args":["literal $(keep) text","--option"]}})).unwrap();
    e
}
fn plan(s: &Service, agent: &str, id: &Value) -> Value {
    s.call("deployment.plan", json!({"target_agent":agent,"mcp_id":id}))
        .unwrap()
}
fn apply(s: &Service, agent: &str, id: &Value, previous: &str) -> Value {
    let p = plan(s, agent, id);
    s.call("deployment.apply",json!({"target_agent":agent,"mcp_id":id,"expected_revision":previous,"plan_digest":p["plan_digest"],"confirm_generated_home":true})).unwrap()
}
#[test]
fn all_adapters_generate_native_files_and_clean_environment_plans_without_credentials_or_execution()
{
    let t = tempfile::tempdir().unwrap();
    let s = Service::open(&t.path().join("vault"), Policy::local_user()).unwrap();
    let e = definition(&s, "fixture");
    for descriptor in Registry::default().list() {
        let agent = descriptor.id;
        let preview = plan(&s, agent, &e["id"]);
        assert!(!s.store.data_dir.join("managed-homes").join(agent).exists());
        assert_eq!(preview["personal_config_written"], false);
        let selected = apply(&s, agent, &e["id"], "none");
        let home = Path::new(selected["integrity"]["home"].as_str().unwrap());
        let path = home.join(selected["active"]["relative_path"].as_str().unwrap());
        let text = fs::read_to_string(&path).unwrap();
        if agent == "codex" {
            let parsed: toml::Value = toml::from_str(&text).unwrap();
            assert_eq!(
                parsed["mcp_servers"]["fixture"]["args"][0]
                    .as_str()
                    .unwrap(),
                "literal $(keep) text"
            );
        } else {
            let parsed: Value = serde_json::from_str(&text).unwrap();
            let entries = parsed
                .get("mcpServers")
                .or_else(|| parsed.get("mcp_servers"))
                .unwrap();
            assert_eq!(
                entries["fixture"]["command"],
                s.store.data_dir.join("fixture-command").to_str().unwrap()
            );
        }
        let launch=s.call("deployment.launch_plan",json!({"target_agent":agent,"expected_revision":selected["active"]["revision"],"cwd":t.path(),"prompt":"Use literal context"})).unwrap();
        for field in [
            "executed",
            "inherit_env",
            "security_isolation",
            "account_authentication_verified",
        ] {
            assert_eq!(launch[field], false);
        }
        #[cfg(unix)]
        {
            let env = launch["env"].as_object().unwrap();
            let output=std::process::Command::new("/bin/sh").args(["-c","test -f \"$HOME/$1\" || exit 2; test -z \"$CONTINUO_FIXTURE_SECRET\" || exit 3; printf ok","fixture",selected["active"]["relative_path"].as_str().unwrap()])
                .env("CONTINUO_FIXTURE_SECRET","synthetic-before-clear").env_clear()
                .envs(env.iter().map(|(k,v)|(k,v.as_str().unwrap()))).output().unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"ok");
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        }
    }
    assert_eq!(s.store.events().unwrap().len(), 1);
    assert!(!serde_json::to_string(&s.store.events().unwrap())
        .unwrap()
        .contains("managed-homes"));
}
#[test]
fn reviewed_plan_cas_rollback_and_external_modifications_preserve_all_generations() {
    let t = tempfile::tempdir().unwrap();
    let s = Service::open(&t.path().join("vault"), Policy::local_user()).unwrap();
    let a = definition(&s, "one");
    let b = definition(&s, "two");
    let pa = plan(&s, "codex", &a["id"]);
    assert_eq!(s.call("deployment.apply",json!({"target_agent":"codex","mcp_id":a["id"],"expected_revision":"none","plan_digest":pa["plan_digest"],"confirm_generated_home":false})).unwrap_err().code,"deployment_confirmation_required");
    let first = apply(&s, "codex", &a["id"], "none");
    let r1 = first["active"]["revision"].as_str().unwrap();
    let second = apply(&s, "codex", &b["id"], r1);
    let r2 = second["active"]["revision"].as_str().unwrap();
    assert_ne!(first["integrity"]["home"], second["integrity"]["home"]);
    assert_eq!(
        s.call(
            "deployment.rollback",
            json!({"target_agent":"codex","expected_revision":r1,"target_revision":r1})
        )
        .unwrap_err()
        .code,
        "deployment_revision_conflict"
    );
    let edited = Path::new(second["integrity"]["home"].as_str().unwrap())
        .join(second["active"]["relative_path"].as_str().unwrap());
    fs::write(&edited, "External change retained").unwrap();
    assert_eq!(
        s.call(
            "deployment.launch_plan",
            json!({"target_agent":"codex","expected_revision":r2,"cwd":t.path()})
        )
        .unwrap_err()
        .code,
        "managed_config_modified"
    );
    let restored = s
        .call(
            "deployment.rollback",
            json!({"target_agent":"codex","expected_revision":r2,"target_revision":r1}),
        )
        .unwrap();
    assert_eq!(
        restored["active"]["generation"],
        first["active"]["generation"]
    );
    assert_eq!(
        restored["previous_integrity_error"]["code"],
        "managed_config_modified"
    );
    assert_eq!(
        fs::read_to_string(edited).unwrap(),
        "External change retained"
    );
    assert_ne!(restored["active"]["revision"], r1);
    let stale_plan = plan(&s, "codex", &a["id"]);
    s.call(
        "entity.update",
        json!({"id":a["id"],"expected_revision":a["heads"][0]["revision"],"name":"Changed"}),
    )
    .unwrap();
    assert_eq!(s.call("deployment.apply",json!({"target_agent":"codex","mcp_id":a["id"],"expected_revision":restored["active"]["revision"],"plan_digest":stale_plan["plan_digest"],"confirm_generated_home":true})).unwrap_err().code,"deployment_plan_changed");
    assert_eq!(s.call("deployment.launch_plan",json!({"target_agent":"codex","expected_revision":restored["active"]["revision"],"cwd":t.path()})).unwrap_err().code,"deployment_source_changed");
    assert_eq!(
        s.call("deployment.inspect", json!({"target_agent":"codex"}))
            .unwrap()["active"]["revision"],
        restored["active"]["revision"]
    );
}
#[test]
fn policy_and_symlink_guards_do_not_write_outside_managed_home() {
    let t = tempfile::tempdir().unwrap();
    let s = Service::open(&t.path().join("vault"), Policy::local_user()).unwrap();
    let e = definition(&s, "fixture");
    let p = plan(&s, "hermes", &e["id"]);
    let readonly = Service::open(&s.store.data_dir, Policy::read_only()).unwrap();
    assert_eq!(readonly.call("deployment.apply",json!({"target_agent":"hermes","mcp_id":e["id"],"expected_revision":"none","plan_digest":p["plan_digest"],"confirm_generated_home":true})).unwrap_err().code,"permission_denied");
    #[cfg(unix)]
    {
        let outside = t.path().join("fake-personal-home");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), "Keep").unwrap();
        std::os::unix::fs::symlink(&outside, s.store.data_dir.join("managed-homes")).unwrap();
        assert_eq!(s.call("deployment.apply",json!({"target_agent":"hermes","mcp_id":e["id"],"expected_revision":"none","plan_digest":p["plan_digest"],"confirm_generated_home":true})).unwrap_err().code,"unsafe_managed_path");
        assert_eq!(
            fs::read_to_string(outside.join("sentinel")).unwrap(),
            "Keep"
        );
        assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
    }
}

#[test]
fn staging_io_failure_keeps_previous_selection_and_cleans_incomplete_home() {
    use continuo_core::adapters::{
        AgentAdapter, Descriptor, LaunchPlan, LaunchRequest, ManagedConfig,
    };
    struct Collision(Registry);
    impl AgentAdapter for Collision {
        fn descriptor(&self) -> Descriptor {
            self.0.get("codex").unwrap().descriptor()
        }
        fn prepare_launch(&self, r: &LaunchRequest) -> continuo_core::Result<LaunchPlan> {
            self.0.get("codex").unwrap().prepare_launch(r)
        }
        fn mcp_registration(&self, e: &str, a: &[String]) -> Value {
            self.0.get("codex").unwrap().mcp_registration(e, a)
        }
        fn prepare_mcp(
            &self,
            k: &str,
            e: &str,
            a: &[String],
            cwd: Option<&str>,
        ) -> continuo_core::Result<Value> {
            self.0.get("codex").unwrap().prepare_mcp(k, e, a, cwd)
        }
        fn prepare_managed_config(&self, r: &Value) -> continuo_core::Result<ManagedConfig> {
            let mut p = self.0.get("codex").unwrap().prepare_managed_config(r)?;
            // An adapter contract fixture forces a directory/file collision during staging.
            p.relative_path = ".codex".into();
            Ok(p)
        }
    }
    let t = tempfile::tempdir().unwrap();
    let mut s = Service::open(&t.path().join("vault"), Policy::local_user()).unwrap();
    let e = definition(&s, "fixture");
    let selected = apply(&s, "codex", &e["id"], "none");
    let old_revision = selected["active"]["revision"].clone();
    let old_home = selected["integrity"]["home"].clone();
    s.registry = Registry::new(vec![Box::new(Collision(Registry::default()))]).unwrap();
    let proposed = plan(&s, "codex", &e["id"]);
    assert_eq!(s.call("deployment.apply",json!({"target_agent":"codex","mcp_id":e["id"],"expected_revision":old_revision,"plan_digest":proposed["plan_digest"],"confirm_generated_home":true})).unwrap_err().code,"io_error");
    let current = s
        .call("deployment.inspect", json!({"target_agent":"codex"}))
        .unwrap();
    assert_eq!(current["active"]["revision"], old_revision);
    assert_eq!(current["integrity"]["home"], old_home);
    assert_eq!(
        fs::read_dir(s.store.data_dir.join("managed-homes/codex/generations"))
            .unwrap()
            .count(),
        1
    );
}
