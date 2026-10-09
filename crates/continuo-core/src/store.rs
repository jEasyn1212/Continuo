use crate::{
    model::{EntityView, Event, KINDS},
    Error, Result,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub struct Store {
    pub(crate) conn: Connection,
    pub data_dir: PathBuf,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if path.exists()
            && !path.join("continuo.sqlite").exists()
            && std::fs::read_dir(path)?.next().is_some()
        {
            return Err(Error::new(
                "invalid_data_directory",
                "Choose an empty directory or an existing Continuo vault",
            ));
        }
        std::fs::create_dir_all(path)?;
        private_permissions(path, true)?;
        let conn = Connection::open(path.join("continuo.sqlite"))?;
        private_permissions(&path.join("continuo.sqlite"), false)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 1 {
            return Err(Error::new(
                "unsupported_schema",
                "Database was created by a newer Continuo",
            ));
        }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS events (revision TEXT PRIMARY KEY, entity_id TEXT NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL); CREATE INDEX IF NOT EXISTS events_entity ON events(entity_id); CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL); PRAGMA user_version=1;")?;
        conn.execute(
            "INSERT OR IGNORE INTO metadata(key,value) VALUES ('device_id',?1)",
            [Uuid::new_v4().to_string()],
        )?;
        Ok(Self {
            conn,
            data_dir: path.to_path_buf(),
        })
    }
    pub fn device_id(&self) -> Result<String> {
        Ok(self.conn.query_row(
            "SELECT value FROM metadata WHERE key='device_id'",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn metadata(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM metadata WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }
    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO metadata(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value])?;
        Ok(())
    }
    pub fn events(&self) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT payload FROM events ORDER BY revision")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn list(&self, kind: Option<&str>, include_deleted: bool) -> Result<Vec<EntityView>> {
        if kind.is_some_and(|k| !KINDS.contains(&k)) {
            return Err(Error::new("invalid_kind", "Unknown entity kind"));
        }
        let mut groups: BTreeMap<String, Vec<Event>> = BTreeMap::new();
        for event in self.events()? {
            if kind.is_none_or(|k| event.kind == k) {
                groups
                    .entry(event.entity_id.clone())
                    .or_default()
                    .push(event);
            }
        }
        Ok(groups
            .into_iter()
            .filter_map(|(id, events)| {
                let parents: BTreeSet<&str> = events
                    .iter()
                    .flat_map(|e| e.parents.iter().map(String::as_str))
                    .collect();
                let heads: Vec<Event> = events
                    .iter()
                    .filter(|e| !parents.contains(e.revision.as_str()))
                    .cloned()
                    .collect();
                if heads.is_empty() || (!include_deleted && heads.iter().all(|e| e.deleted)) {
                    return None;
                }
                Some(EntityView {
                    id,
                    kind: heads[0].kind.clone(),
                    conflicted: heads.len() > 1,
                    heads,
                })
            })
            .collect())
    }
    pub fn get(&self, id: &str) -> Result<EntityView> {
        let mut stmt = self
            .conn
            .prepare("SELECT payload FROM events WHERE entity_id=?1 ORDER BY revision")?;
        let rows = stmt.query_map([id], |r| r.get::<_, String>(0))?;
        let events: Vec<Event> = rows
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect::<Result<_>>()?;
        let parents: BTreeSet<&str> = events
            .iter()
            .flat_map(|e| e.parents.iter().map(String::as_str))
            .collect();
        let heads: Vec<Event> = events
            .iter()
            .filter(|e| !parents.contains(e.revision.as_str()))
            .cloned()
            .collect();
        if heads.is_empty() {
            return Err(Error::new("not_found", "Entity does not exist"));
        }
        Ok(EntityView {
            id: id.into(),
            kind: heads[0].kind.clone(),
            conflicted: heads.len() > 1,
            heads,
        })
    }
    pub fn create(&self, kind: &str, name: &str, data: Value) -> Result<EntityView> {
        let event = self.new_event(Uuid::new_v4().to_string(), kind, name, data, false, vec![])?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        crate::identity::validate_local(self, &event)?;
        crate::task::validate_local(self, &event)?;
        crate::session::validate_local(self, &event)?;
        crate::capability::validate_local(self, &event)?;
        self.insert(&event)?;
        tx.commit()?;
        self.get(&event.entity_id)
    }
    pub fn update(
        &self,
        id: &str,
        expected: &str,
        name: Option<&str>,
        data: Option<Value>,
        deleted: bool,
    ) -> Result<EntityView> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let view = self.get(id)?;
        if view.heads.len() != 1 || view.heads[0].revision != expected || view.heads[0].deleted {
            return Err(Error::new(
                "revision_conflict",
                "Entity changed, was deleted, or has unresolved conflicts",
            )
            .details(json!({"current":view})));
        }
        let old = &view.heads[0];
        let event = self.new_event(
            id.into(),
            &old.kind,
            name.unwrap_or(&old.name),
            data.unwrap_or_else(|| old.data.clone()),
            deleted,
            vec![expected.into()],
        )?;
        crate::identity::validate_local(self, &event)?;
        crate::task::validate_local(self, &event)?;
        crate::session::validate_local(self, &event)?;
        crate::capability::validate_local(self, &event)?;
        self.insert(&event)?;
        tx.commit()?;
        self.get(id)
    }
    pub fn resolve(
        &self,
        id: &str,
        expected: &[String],
        name: &str,
        data: Value,
        deleted: bool,
    ) -> Result<EntityView> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let view = self.get(id)?;
        let actual: BTreeSet<String> = view.heads.iter().map(|e| e.revision.clone()).collect();
        if actual.len() < 2
            || actual != expected.iter().cloned().collect()
            || expected.len() != actual.len()
        {
            return Err(Error::new(
                "revision_conflict",
                "Resolution must reference exactly all current conflict heads",
            )
            .details(json!({"current":view})));
        }
        let event = self.new_event(
            id.into(),
            &view.kind,
            name,
            data,
            deleted,
            expected.to_vec(),
        )?;
        crate::identity::validate_local(self, &event)?;
        crate::task::validate_local(self, &event)?;
        crate::session::validate_local(self, &event)?;
        crate::capability::validate_local(self, &event)?;
        self.insert(&event)?;
        tx.commit()?;
        self.get(id)
    }
    fn history_events(&self, id: &str) -> Result<Vec<Event>> {
        self.get(id)?;
        let mut stmt = self
            .conn
            .prepare("SELECT payload FROM events WHERE entity_id=?1 ORDER BY revision")?;
        let rows = stmt.query_map([id], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn history_version(&self, id: &str, revision: &str) -> Result<Event> {
        self.history_events(id)?
            .into_iter()
            .find(|e| e.revision == revision)
            .ok_or_else(|| {
                Error::new(
                    "history_version_missing",
                    "Version does not belong to this entity",
                )
            })
    }
    pub fn history(&self, id: &str) -> Result<Value> {
        let events = self.history_events(id)?;
        let mut ready: BTreeSet<String> = events
            .iter()
            .filter(|e| e.parents.is_empty())
            .map(|e| e.revision.clone())
            .collect();
        let mut degree: BTreeMap<String, usize> = events
            .iter()
            .map(|e| (e.revision.clone(), e.parents.len()))
            .collect();
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for e in &events {
            for p in &e.parents {
                children
                    .entry(p.clone())
                    .or_default()
                    .push(e.revision.clone());
            }
        }
        let by_revision: BTreeMap<_, _> = events.iter().map(|e| (e.revision.clone(), e)).collect();
        let mut ordered = vec![];
        while let Some(revision) = ready.pop_first() {
            let e = by_revision[&revision];
            ordered.push(json!({"revision":e.revision,"name":e.name,"deleted":e.deleted,"parents":if e.parents.len()<=128{Some(&e.parents)}else{None},"parent_count":e.parents.len(),"device_id":e.device_id,"timestamp_ms":e.timestamp_ms}));
            if let Some(items) = children.get(&revision) {
                for child in items {
                    let d = degree.get_mut(child).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        ready.insert(child.clone());
                    }
                }
            }
        }
        if ordered.len() != events.len() {
            return Err(Error::new(
                "invalid_event",
                "Local entity history is incomplete or cyclic",
            ));
        }
        let truncated = ordered.len() > 200;
        let records = ordered.into_iter().rev().take(200).collect::<Vec<_>>();
        Ok(
            json!({"id":id,"versions":records,"total_versions":events.len(),"truncated":truncated,"order":"reverse_causal_topology_uuid_ties","timestamp_is_display_only":true}),
        )
    }
    pub fn restore(&self, id: &str, expected: &str, source: &str) -> Result<EntityView> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let view = self.get(id)?;
        if view.conflicted || !view.heads[0].deleted || view.heads[0].revision != expected {
            return Err(Error::new(
                "revision_conflict",
                "Restore requires the exact single current tombstone",
            ));
        }
        let source = self.history_version(id, source)?;
        if source.deleted {
            return Err(Error::new(
                "invalid_restore_source",
                "Choose a nondeleted historical version",
            ));
        }
        let event = self.new_event(
            id.into(),
            &view.kind,
            &source.name,
            source.data,
            false,
            vec![expected.into()],
        )?;
        crate::identity::validate_local(self, &event)?;
        crate::task::validate_local(self, &event)?;
        crate::session::validate_local(self, &event)?;
        crate::capability::validate_local(self, &event)?;
        self.insert(&event)?;
        tx.commit()?;
        self.get(id)
    }
    fn new_event(
        &self,
        id: String,
        kind: &str,
        name: &str,
        data: Value,
        deleted: bool,
        parents: Vec<String>,
    ) -> Result<Event> {
        let event = Event {
            schema_version: 1,
            revision: Uuid::new_v4().to_string(),
            entity_id: id,
            kind: kind.into(),
            name: name.into(),
            data,
            deleted,
            parents,
            device_id: self.device_id()?,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };
        event.validate()?;
        Ok(event)
    }
    fn insert(&self, event: &Event) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events(revision,entity_id,kind,payload) VALUES (?1,?2,?3,?4)",
            params![
                event.revision,
                event.entity_id,
                event.kind,
                serde_json::to_string(event)?
            ],
        )?;
        Ok(())
    }
    /// Validate the complete causal graph before committing any remote event.
    pub fn validate_import(&self, incoming: &[Event]) -> Result<usize> {
        self.import_checked(incoming, false)
    }
    pub fn import_events(&self, incoming: &[Event]) -> Result<usize> {
        self.import_checked(incoming, true)
    }
    fn import_checked(&self, incoming: &[Event], write: bool) -> Result<usize> {
        let tx = Transaction::new_unchecked(
            &self.conn,
            if write {
                TransactionBehavior::Immediate
            } else {
                TransactionBehavior::Deferred
            },
        )?;
        let existing = self.events()?;
        let mut graph: BTreeMap<String, Event> = existing
            .into_iter()
            .map(|e| (e.revision.clone(), e))
            .collect();
        let mut added = Vec::new();
        for event in incoming {
            event.validate()?;
            if let Some(old) = graph.get(&event.revision) {
                if old != event {
                    return Err(Error::new(
                        "event_collision",
                        "Revision has different content",
                    ));
                }
            } else {
                graph.insert(event.revision.clone(), event.clone());
                added.push(event);
            }
        }
        let mut entity_kinds = BTreeMap::new();
        for event in graph.values() {
            if let Some(kind) = entity_kinds.insert(&event.entity_id, &event.kind) {
                if kind != &event.kind {
                    return Err(Error::new("invalid_event", "Entity kind cannot change"));
                }
            }
            for parent in &event.parents {
                let p = graph
                    .get(parent)
                    .ok_or_else(|| Error::new("missing_parent", "Incomplete event history"))?;
                if p.entity_id != event.entity_id || p.kind != event.kind {
                    return Err(Error::new(
                        "invalid_event",
                        "Parent belongs to a different entity",
                    ));
                }
            }
        }
        // Topological traversal detects cycles without recursion or timestamp assumptions.
        let mut degrees: BTreeMap<&str, usize> = graph
            .values()
            .map(|e| (e.revision.as_str(), e.parents.len()))
            .collect();
        let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut ready: VecDeque<&str> = degrees
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(r, _)| *r)
            .collect();
        for event in graph.values() {
            for parent in &event.parents {
                children
                    .entry(parent.as_str())
                    .or_default()
                    .push(event.revision.as_str());
            }
        }
        let mut visited = 0;
        while let Some(revision) = ready.pop_front() {
            visited += 1;
            if let Some(items) = children.get(revision) {
                for child in items {
                    let degree = degrees.get_mut(child).expect("validated event graph");
                    *degree -= 1;
                    if *degree == 0 {
                        ready.push_back(child);
                    }
                }
            }
        }
        if visited != graph.len() {
            return Err(Error::new(
                "invalid_event",
                "Event history contains a cycle",
            ));
        }
        if write {
            for event in &added {
                self.insert(event)?;
            }
        }
        tx.commit()?;
        Ok(added.len())
    }
    pub fn status(&self) -> Result<Value> {
        let entities = self.list(None, true)?;
        let counts: BTreeMap<&str, usize> = KINDS
            .iter()
            .map(|k| {
                (
                    *k,
                    entities
                        .iter()
                        .filter(|e| e.kind == *k && !e.heads.iter().all(|h| h.deleted))
                        .count(),
                )
            })
            .collect();
        Ok(
            json!({"device_id":self.device_id()?,"counts":counts,"conflicts":entities.iter().filter(|e|e.conflicted).count(),"event_count":self.events()?.len(),"sync_configured":self.metadata("sync_config")?.is_some(),"last_sync":self.metadata("last_sync")?}),
        )
    }
}

pub(crate) fn private_permissions(path: &Path, dir: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if dir { 0o700 } else { 0o600 }),
        )?;
    }
    #[cfg(not(unix))]
    let _ = (path, dir);
    Ok(())
}
