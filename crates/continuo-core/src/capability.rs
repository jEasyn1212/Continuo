//! Portable, text-only capabilities. Reviewing content never grants runtime permissions.
use crate::{adapters::AgentAdapter, model::Event, store::Store, Error, Result};
use rusqlite::{Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub capability_type: String,
    pub description: String,
    pub body: String,
    pub version: String,
    pub source_ref: Option<String>,
    pub source_revision: Option<String>,
    pub source_license: Option<String>,
    pub agent_targets: Vec<String>,
    pub requires: Vec<String>,
    pub reviewed_digest: Option<String>,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            capability_type: "rule".into(),
            description: String::new(),
            body: String::new(),
            version: "draft".into(),
            source_ref: None,
            source_revision: None,
            source_license: None,
            agent_targets: vec![],
            requires: vec![],
            reviewed_digest: None,
        }
    }
}
impl Profile {
    pub fn parse(data: &Value) -> Result<Self> {
        let p: Self = serde_json::from_value(data.clone()).map_err(|_| {
            Error::new(
                "invalid_capability_profile",
                "Invalid capability field types",
            )
        })?;
        if !["rule", "skill"].contains(&p.capability_type.as_str())
            || p.description.len() > 8192
            || p.body.len() > 65536
            || p.body.contains('\0')
            || p.version.trim().is_empty()
            || p.version.len() > 256
            || p.source_ref
                .as_ref()
                .is_some_and(|s| !crate::task::reference(s))
            || [&p.source_revision, &p.source_license].iter().any(|v| {
                v.as_ref().is_some_and(|s| {
                    s.trim().is_empty() || s.len() > 1024 || s.chars().any(char::is_control)
                })
            })
            || (p.source_ref.is_none() && p.source_revision.is_some())
            || p.reviewed_digest.as_ref().is_some_and(|s| {
                s.len() != 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err(Error::new("invalid_capability_profile", "Type must be rule/skill; body max 64 KiB, version max 256 bytes; sources must be portable credential-free references"));
        }
        let mut ids = BTreeSet::new();
        if p.requires.len() > 64
            || p.requires.iter().any(|id| {
                Uuid::parse_str(id).ok().map(|u| u.to_string()).as_ref() != Some(id)
                    || !ids.insert(id)
            })
        {
            return Err(Error::new(
                "invalid_capability_profile",
                "Dependencies require at most 64 unique canonical UUIDs",
            ));
        }
        ids.clear();
        if p.agent_targets.len() > 64
            || p.agent_targets.iter().any(|id| {
                id.is_empty()
                    || id.len() > 128
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    || !ids.insert(id)
            })
        {
            return Err(Error::new(
                "invalid_capability_profile",
                "Agent targets must be unique adapter identifiers",
            ));
        }
        Ok(p)
    }
}
pub fn digest(event: &Event) -> Result<String> {
    let mut p = Profile::parse(&event.data)?;
    p.reviewed_digest = None;
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&json!({"name":event.name,"profile":p}))?)
    ))
}
fn live(store: &Store, id: &str) -> Result<Event> {
    let view = store.get(id)?;
    if view.kind != "capability" || view.conflicted || view.heads[0].deleted {
        return Err(Error::new(
            "capability_unavailable",
            "Capability has the wrong kind, is deleted, or has concurrent versions",
        )
        .details(json!({"record":view})));
    }
    Ok(view.heads[0].clone())
}
struct Graph {
    events: BTreeMap<String, Event>,
    order: Vec<String>,
    active: BTreeSet<String>,
    issues: Vec<Value>,
}
impl Graph {
    fn new() -> Self {
        Self {
            events: BTreeMap::new(),
            order: vec![],
            active: BTreeSet::new(),
            issues: vec![],
        }
    }
    fn visit(&mut self, store: &Store, id: &str, target: Option<&str>, depth: usize) -> Result<()> {
        if self.active.contains(id) {
            self.issues.push(json!({"id":id,"code":"dependency_cycle"}));
            return Ok(());
        }
        if self.events.contains_key(id) {
            return Ok(());
        }
        if depth > 64 || self.events.len() >= 128 {
            return Err(Error::new(
                "capability_graph_too_large",
                "Capability graph exceeds 128 records or 64 levels",
            ));
        }
        let e = match live(store, id) {
            Ok(e) => e,
            Err(e) if ["not_found", "capability_unavailable"].contains(&e.code.as_str()) => {
                self.issues
                    .push(json!({"id":id,"code":e.code,"details":e.details}));
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let p = Profile::parse(&e.data)?;
        if p.body.trim().is_empty() {
            self.issues.push(json!({"id":id,"code":"body_missing"}));
        }
        if p.reviewed_digest.as_deref() != Some(&digest(&e)?) {
            self.issues.push(json!({"id":id,"code":"review_required"}));
        }
        if target
            .is_some_and(|a| !p.agent_targets.is_empty() && !p.agent_targets.iter().any(|t| t == a))
        {
            self.issues
                .push(json!({"id":id,"code":"agent_incompatible","agent":target}));
        }
        self.events.insert(id.into(), e);
        self.active.insert(id.into());
        for dep in p.requires {
            self.visit(store, &dep, target, depth + 1)?;
        }
        self.active.remove(id);
        self.order.push(id.into());
        Ok(())
    }
    fn records(&self) -> Vec<&Event> {
        self.order.iter().map(|id| &self.events[id]).collect()
    }
}
pub fn inspect(store: &Store, id: &str, target: Option<&str>) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred)?;
    let event = live(store, id)?;
    let mut graph = Graph::new();
    graph.visit(store, id, target, 0)?;
    let result = json!({"capability":event,"profile":Profile::parse(&event.data)?,"digest":digest(&event)?,"ready":graph.issues.is_empty(),"issues":graph.issues,"resolved":graph.records(),"target_agent":target,"source_verified":false,"scripts_supported":false,"permissions_granted":false});
    tx.commit()?;
    Ok(result)
}
pub fn review(store: &Store, id: &str, expected: &str, expected_digest: &str) -> Result<Value> {
    let event = live(store, id)?;
    if event.revision != expected || digest(&event)? != expected_digest {
        return Err(Error::new(
            "revision_conflict",
            "Capability changed; inspect the exact content again before reviewing",
        ));
    }
    if Profile::parse(&event.data)?.body.trim().is_empty() {
        return Err(Error::new(
            "capability_not_ready",
            "Add capability body before review",
        ));
    }
    let mut data = event.data;
    data["reviewed_digest"] = json!(expected_digest);
    Ok(serde_json::to_value(store.update(
        id,
        expected,
        None,
        Some(data),
        false,
    )?)?)
}
/// Used by the adapter contract: text is passed as context, never as a shell command.
pub fn prepare(store: &Store, roots: &[String], adapter: &dyn AgentAdapter) -> Result<Value> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred)?;
    let mut graph = Graph::new();
    for id in roots {
        graph.visit(store, id, Some(adapter.descriptor().id), 0)?;
    }
    if !graph.issues.is_empty() {
        return Err(Error::new("capability_not_ready", "Repair, review and check agent compatibility for every selected capability and dependency").details(json!({"issues":graph.issues})));
    }
    let mut applications = vec![];
    let mut total = 0;
    for event in graph.records() {
        let p = Profile::parse(&event.data)?;
        total += p.body.len();
        if total > 131072 {
            return Err(Error::new(
                "capability_graph_too_large",
                "Combined capability content exceeds 128 KiB",
            ));
        }
        applications.push(adapter.prepare_capability(event, &p)?);
    }
    let result = json!({"roots":roots,"applications":applications,"scripts_executed":false,"permissions_granted":false,"config_written":false});
    tx.commit()?;
    Ok(result)
}
/// Local dependency edits enforce liveness and cycles; imported incomplete graphs remain repairable.
pub(crate) fn validate_local(store: &Store, event: &Event) -> Result<()> {
    if event.kind != "capability" || event.deleted {
        return Ok(());
    }
    let p = Profile::parse(&event.data)?;
    let mut seen = BTreeSet::new();
    let mut active = BTreeSet::from([event.entity_id.clone()]);
    fn visit(
        store: &Store,
        id: &str,
        seen: &mut BTreeSet<String>,
        active: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<()> {
        if active.contains(id) {
            return Err(Error::new(
                "capability_dependency_cycle",
                "Capability dependencies must not contain cycles",
            ));
        }
        if seen.contains(id) {
            return Ok(());
        }
        if depth > 64 || seen.len() >= 128 {
            return Err(Error::new(
                "capability_graph_too_large",
                "Capability graph is too large",
            ));
        }
        let event = live(store, id).map_err(|e| {
            Error::new("capability_dependency_unavailable", e.message).details(json!({"id":id}))
        })?;
        active.insert(id.into());
        for dep in Profile::parse(&event.data)?.requires {
            visit(store, &dep, seen, active, depth + 1)?;
        }
        active.remove(id);
        seen.insert(id.into());
        Ok(())
    }
    for dep in p.requires {
        visit(store, &dep, &mut seen, &mut active, 0)?;
    }
    Ok(())
}
