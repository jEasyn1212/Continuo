use continuo_core::{
    api::{Policy, Service},
    model::Event,
};
use serde_json::json;

#[test]
fn portable_objects_reject_nested_local_paths_accounts_and_runtime_history_atomically() {
    let root = tempfile::tempdir().unwrap();
    let service = Service::open(&root.path().join("vault"), Policy::local_user()).unwrap();
    for kind in ["identity", "task", "capability", "device"] {
        for data in [
            json!({"cwd":root.path()}),
            json!({"extension":{"LOCAL-PATH":root.path()}}),
            json!({"extension":{"messages":[{"role":"user","content":"synthetic runtime fixture"}]}}),
            json!({"account_ref":"credential:fixture"}),
        ] {
            assert_eq!(
                service
                    .call(
                        "entity.create",
                        json!({"kind":kind,"name":"local-only","data":data})
                    )
                    .unwrap_err()
                    .code,
                "nonportable_data"
            );
        }
    }
    assert!(service.store.events().unwrap().is_empty());
    let safe=service.call("entity.create",json!({"kind":"task","name":"safe","data":{"goal":"Portable work","custom":{"note":"A preserved extension"}}})).unwrap();
    assert_eq!(service.call("entity.update",json!({"id":safe["id"],"expected_revision":safe["heads"][0]["revision"],"data":{"transcript":"synthetic"}})).unwrap_err().code,"nonportable_data");
    assert_eq!(service.store.events().unwrap().len(), 1);
    let mut valid: Event = serde_json::from_value(safe["heads"][0].clone()).unwrap();
    valid.entity_id = uuid::Uuid::new_v4().to_string();
    valid.revision = uuid::Uuid::new_v4().to_string();
    valid.parents = vec![];
    let mut invalid = valid.clone();
    invalid.entity_id = uuid::Uuid::new_v4().to_string();
    invalid.revision = uuid::Uuid::new_v4().to_string();
    invalid.data = json!({"runtime_state":{}});
    assert_eq!(
        service
            .store
            .import_events(&[valid, invalid])
            .unwrap_err()
            .code,
        "nonportable_data"
    );
    assert_eq!(service.store.events().unwrap().len(), 1);
}

#[test]
fn secret_field_spelling_cannot_bypass_recursive_portability_validation() {
    let root = tempfile::tempdir().unwrap();
    let service = Service::open(&root.path().join("vault"), Policy::local_user()).unwrap();
    for key in [
        "accessToken",
        "refreshToken",
        "privateKey",
        "API-KEY",
        "access_token",
    ] {
        assert_eq!(service.call("entity.create", json!({"kind":"device","name":"fixture","data":{"extension":{key:"synthetic-not-a-secret"}}})).unwrap_err().code, "credential_not_allowed");
    }
    assert!(service.store.events().unwrap().is_empty());
}
