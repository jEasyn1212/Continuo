//! Portable identity profiles; the active selection is device-local metadata.
use crate::{model::Event, store::Store, Error, Result};
use rusqlite::{Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub description: String,
    pub instructions: String,
    pub preferred_agent: Option<String>,
    pub capability_ids: Vec<String>,
    pub mcp_ids: Vec<String>,
}
impl Profile {
    pub fn parse(data: &Value) -> Result<Self> {
        let profile: Self = serde_json::from_value(data.clone()).map_err(|_| {
            Error::new(
                "invalid_identity_profile",
                "Identity fields have invalid types",
            )
        })?;
        if profile.description.len() > 8192
            || profile.instructions.len() > 65536
            || profile
                .preferred_agent
                .as_ref()
                .is_some_and(|id| id.is_empty() || id.len() > 128)
        {
            return Err(Error::new(
                "invalid_identity_profile",
                "Description max 8 KiB; instructions max 64 KiB; agent identifier max 128 bytes",
            ));
        }
        for ids in [&profile.capability_ids, &profile.mcp_ids] {
            let mut seen = BTreeSet::new();
            if ids.len() > 64
                || ids.iter().any(|id| {
                    Uuid::parse_str(id).ok().map(|u| u.to_string()).as_ref() != Some(id)
                        || !seen.insert(id)
                })
            {
                return Err(Error::new(
                    "invalid_identity_profile",
                    "Bindings must contain at most 64 unique canonical UUIDs per kind",
                ));
            }
        }
        Ok(profile)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Selection {
    pub revision: String,
    pub identity_id: Option<String>,
}
fn selection(store: &Store) -> Result<Selection> {
    match store.metadata("active_identity")? {
        Some(value) => Ok(serde_json::from_str(&value)?),
        None => Ok(Selection {
            revision: "none".into(),
            identity_id: None,
        }),
    }
}
fn live(store: &Store, id: &str) -> Result<Event> {
    let view = store.get(id)?;
    if view.kind != "identity" || view.conflicted || view.heads[0].deleted {
        return Err(Error::new(
            "invalid_identity",
            "Choose a live identity without unresolved conflicts",
        ));
    }
    Ok(view.heads[0].clone())
}
pub(crate) fn inspect_inner(store: &Store, id: &str) -> Result<Value> {
    let event = live(store, id)?;
    let profile = Profile::parse(&event.data)?;
    let mut bindings = vec![];
    let mut ready = true;
    for (kind, ids) in [
        ("capability", &profile.capability_ids),
        ("mcp", &profile.mcp_ids),
    ] {
        for id in ids {
            let binding = match store.get(id) {
                Err(e) if e.code == "not_found" => json!({"id":id,"kind":kind,"state":"missing"}),
                Err(e) => return Err(e),
                Ok(view) => {
                    let state = if view.kind != kind {
                        "wrong_kind"
                    } else if view.conflicted {
                        "conflicted"
                    } else if view.heads[0].deleted {
                        "deleted"
                    } else {
                        "ready"
                    };
                    json!({"id":id,"kind":kind,"state":state,"record":view})
                }
            };
            ready &= binding["state"] == "ready";
            bindings.push(binding);
        }
    }
    Ok(
        json!({"identity":event,"profile":profile,"bindings":bindings,"ready":ready,
        "instructions_provide_security_isolation":false,"bindings_deployed":false}),
    )
}
pub fn inspect(store: &Store, id: &str) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred)?;
    let result = inspect_inner(store, id)?;
    tx.commit()?;
    Ok(result)
}
fn current_inner(store: &Store) -> Result<Value> {
    let selected = selection(store)?;
    let (state, context, error) = match &selected.identity_id {
        None => ("none", Value::Null, Value::Null),
        Some(id) => match inspect_inner(store, id) {
            Ok(context) => (
                if context["ready"] == true {
                    "ready"
                } else {
                    "unavailable"
                },
                context,
                Value::Null,
            ),
            Err(e)
                if ["not_found", "invalid_identity", "invalid_identity_profile"]
                    .contains(&e.code.as_str()) =>
            {
                ("unavailable", Value::Null, serde_json::to_value(e)?)
            }
            Err(e) => return Err(e),
        },
    };
    Ok(
        json!({"selection":selected,"state":state,"context":context,"error":error,"scope":"device","device_id":store.device_id()?}),
    )
}
pub fn current(store: &Store) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred)?;
    let result = current_inner(store)?;
    tx.commit()?;
    Ok(result)
}
/// A CAS protects selection changes made by App, Web and multiple agents.
pub fn select(
    store: &Store,
    expected_selection: &str,
    target: Option<(&str, &str)>,
) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    let old = selection(store)?;
    if old.revision != expected_selection {
        return Err(Error::new(
            "selection_conflict",
            "Current identity changed; read identity.current again",
        )
        .details(json!({"selection":old})));
    }
    if let Some((id, expected)) = target {
        let context = inspect_inner(store, id)?;
        if context["identity"]["revision"] != expected {
            return Err(Error::new(
                "revision_conflict",
                "Identity changed before activation",
            ));
        }
        if context["ready"] != true {
            return Err(Error::new(
                "identity_bindings_unavailable",
                "Resolve missing, deleted or conflicted bindings before activation",
            )
            .details(context));
        }
    }
    store.set_metadata(
        "active_identity",
        &serde_json::to_string(&Selection {
            revision: Uuid::new_v4().to_string(),
            identity_id: target.map(|(id, _)| id.to_owned()),
        })?,
    )?;
    let result = current_inner(store)?;
    tx.commit()?;
    Ok(result)
}
/// Local edits enforce binding types and liveness in the write transaction.
/// Sync imports only validate structure: missing dependencies remain visible and block use.
pub(crate) fn validate_local(store: &Store, event: &Event) -> Result<()> {
    if event.kind != "identity" || event.deleted {
        return Ok(());
    }
    let profile = Profile::parse(&event.data)?;
    for (kind, ids) in [
        ("capability", profile.capability_ids),
        ("mcp", profile.mcp_ids),
    ] {
        for id in ids {
            let view = store.get(&id).map_err(|e| {
                if e.code == "not_found" {
                    Error::new("identity_bindings_unavailable", "Binding does not exist")
                } else {
                    e
                }
            })?;
            if view.kind != kind || view.conflicted || view.heads[0].deleted {
                return Err(Error::new(
                    "identity_bindings_unavailable",
                    "Bindings must point to live records of the correct kind without conflicts",
                ));
            }
        }
    }
    Ok(())
}
pub fn launch_context(
    store: &Store,
    explicit: Option<&str>,
    use_current: bool,
) -> Result<Option<Value>> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred)?;
    let (id, source) = if let Some(id) = explicit {
        (Some(id.to_owned()), "explicit")
    } else if use_current {
        (selection(store)?.identity_id, "current")
    } else {
        (None, "none")
    };
    let context = if let Some(id) = id {
        let mut context = inspect_inner(store, &id)?;
        if context["ready"] != true {
            return Err(Error::new(
                "identity_bindings_unavailable",
                "Identity bindings are unavailable; repair the profile before preparing an agent",
            )
            .details(context));
        }
        context["source"] = json!(source);
        Some(context)
    } else {
        None
    };
    tx.commit()?;
    Ok(context)
}
