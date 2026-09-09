//! Offline import of T3's v2 projections. Source reads share one SQLite snapshot;
//! all conversion and matching finishes before any destination write.
mod history;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use agent::{ApprovalMode, InteractionMode, OptionSelection, ProviderKind, ResumeCursor};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use tcode_core::project::{Project, SessionMeta};
use tcode_core::settings::Settings;

use crate::store::SessionStore;

#[derive(Debug)]
pub struct ImportOptions {
    pub source: PathBuf,
    pub data_dir: PathBuf,
    pub profiles: BTreeMap<String, String>,
    pub skip_settled: bool,
    pub skip_archived: bool,
    pub dry_run: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            source: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".t3/userdata"),
            data_dir: SessionStore::default_root(),
            profiles: BTreeMap::new(),
            skip_settled: false,
            skip_archived: false,
            dry_run: false,
        }
    }
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub projects_created: usize,
    pub projects_reused: usize,
    pub threads_created: usize,
    pub threads_refreshed: usize,
    pub exclusions: BTreeMap<String, usize>,
    pub omitted_attachments: usize,
    pub failures: Vec<String>,
}

impl ImportReport {
    fn skip(&mut self, reason: &str) {
        *self.exclusions.entry(reason.into()).or_default() += 1;
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn driver(name: &str) -> Result<ProviderKind, String> {
    match name {
        "codex" => Ok(ProviderKind::Codex),
        "claudeAgent" => Ok(ProviderKind::ClaudeCode),
        _ => Err(format!("unsupported T3 provider driver {name:?}")),
    }
}

fn profiles(
    options: &ImportOptions,
    settings: &Settings,
) -> Result<HashMap<String, (ProviderKind, Option<String>)>, String> {
    let config = read_json(&options.source.join("settings.json"))?;
    let mut instances = HashMap::from([
        ("codex".to_string(), "codex".to_string()),
        ("claudeAgent".to_string(), "claudeAgent".to_string()),
    ]);
    if let Some(custom) = config.get("providerInstances") {
        for (id, value) in custom
            .as_object()
            .ok_or("providerInstances must be an object")?
        {
            instances.insert(id.clone(), required(value, "driver")?.into());
        }
    }
    let mut result = HashMap::from([
        ("codex".into(), (ProviderKind::Codex, None)),
        ("claudeAgent".into(), (ProviderKind::ClaudeCode, None)),
    ]);
    for (source, destination) in &options.profiles {
        let kind = driver(
            instances
                .get(source)
                .ok_or_else(|| format!("unknown T3 instance {source:?}"))?,
        )?;
        let profile = settings
            .resolved_profile(destination)
            .ok_or_else(|| format!("unknown destination profile {destination:?}"))?;
        if profile.kind != kind {
            return Err(format!(
                "profile {destination:?} is incompatible with T3 instance {source:?}"
            ));
        }
        result.insert(
            source.clone(),
            (
                kind,
                (!Settings::is_builtin_profile_id(destination)).then(|| destination.clone()),
            ),
        );
    }
    Ok(result)
}

pub(super) fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing or invalid {key}"))
}

pub(super) fn timestamp(value: &str) -> Result<u64, String> {
    let time = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|e| format!("invalid timestamp {value:?}: {e}"))?;
    u64::try_from(time.timestamp_millis())
        .map_err(|_| format!("timestamp before Unix epoch: {value}"))
}

pub(super) fn payloads(
    db: &Connection,
    table: &str,
    thread_id: Option<&str>,
) -> Result<Vec<Value>, String> {
    // Table names are private constants, never source data or command-line input.
    let mut query = format!("SELECT payload_json FROM orchestration_v2_projection_{table}");
    if thread_id.is_some() {
        query.push_str(" WHERE thread_id = ?1");
    }
    let mut statement = db.prepare(&query).map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(thread_id), |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?;
    rows.map(|row| {
        let text = row.map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| format!("invalid {table} payload: {e}"))
    })
    .collect()
}

struct PreparedThread {
    meta: SessionMeta,
    bytes: Vec<u8>,
    refresh: bool,
}

/// Returns configuration errors before writing; conversion and write failures are
/// listed separately from intentional exclusions in the report.
pub fn import(options: &ImportOptions) -> Result<ImportReport, String> {
    let store = SessionStore::inspect_at(options.data_dir.clone());
    let inspection_lock = store.lock_exclusive(false).map_err(|e| e.to_string())?;
    let mut index = store
        .read_file_strict()
        .map_err(|e| format!("destination index: {e}"))?;
    let original_projects = index.projects.clone();
    let original_sessions = index.sessions.clone();
    let settings: Settings =
        serde_json::from_value(read_json(&options.data_dir.join("settings.json"))?)
            .map_err(|e| format!("destination settings: {e}"))?;
    let profiles = profiles(options, &settings)?;
    let mut db = Connection::open_with_flags(
        options.source.join("state.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| format!("T3 database: {e}"))?;
    let db = db.transaction().map_err(|e| e.to_string())?;
    let version: i64 = db.query_row("SELECT schema_version FROM orchestration_v2_projection_metadata WHERE projection_name = 'thread-projections'", [], |row| row.get(0)).map_err(|e| format!("unsupported T3 schema: {e}"))?;
    if version != 2 {
        return Err(format!(
            "unsupported T3 projection schema {version}; expected 2"
        ));
    }
    // Check required projections even if there are no eligible threads.
    for table in [
        "threads",
        "provider_threads",
        "runs",
        "messages",
        "turn_items",
    ] {
        db.prepare(&format!(
            "SELECT thread_id, payload_json FROM orchestration_v2_projection_{table} LIMIT 0"
        ))
        .map_err(|e| format!("unsupported T3 schema: {e}"))?;
    }
    let mut report = ImportReport::default();
    let mut project_map = HashMap::new();
    let mut query = db.prepare("SELECT project_id, title, workspace_root, created_at, deleted_at FROM projection_projects ORDER BY created_at, project_id").map_err(|e| format!("unsupported T3 projects: {e}"))?;
    let projects = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in projects {
        let (id, title, root, created, deleted) = row.map_err(|e| e.to_string())?;
        if deleted.is_some() {
            report.skip("deleted project");
            continue;
        }
        if !Path::new(&root).is_dir() {
            report.skip("missing project directory");
            continue;
        }
        let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        let matches: Vec<_> = index
            .projects
            .iter()
            .filter(|project| std::fs::canonicalize(&project.root).is_ok_and(|path| path == root))
            .collect();
        let project = match matches.as_slice() {
            [] => {
                let mut project = Project::from_root(root);
                project.name = title;
                project.created_at = timestamp(&created)? / 1000;
                index.projects.push(project.clone());
                report.projects_created += 1;
                project
            }
            [project] => {
                report.projects_reused += 1;
                (*project).clone()
            }
            _ => {
                return Err(format!(
                    "ambiguous destination project path {}",
                    root.display()
                ));
            }
        };
        project_map.insert(id, project);
    }
    let provider_threads = payloads(&db, "provider_threads", None)?;
    let providers: HashMap<_, _> = provider_threads
        .iter()
        .map(|p| Ok((required(p, "id")?, p)))
        .collect::<Result<_, String>>()?;
    let mut threads = payloads(&db, "threads", None)?;
    threads.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let mut prepared = Vec::new();
    let mut claimed = HashSet::new();
    for thread in threads {
        let id = required(&thread, "id")?;
        if !thread["deletedAt"].is_null() {
            report.skip("deleted thread");
            continue;
        }
        if thread["lineage"]["relationshipToParent"] == "subagent" {
            report.skip("subagent activity");
            continue;
        }
        let Some(project) = project_map.get(required(&thread, "projectId")?) else {
            report.skip("excluded project");
            continue;
        };
        let settled = thread["settledOverride"] == "settled";
        if options.skip_settled && settled {
            report.skip("settled thread");
            continue;
        }
        if options.skip_archived && !thread["archivedAt"].is_null() {
            report.skip("archived thread");
            continue;
        }
        let cwd = thread["worktreePath"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| project.root.clone());
        if !cwd.is_dir() {
            report.skip("missing thread directory");
            continue;
        }
        let Some(provider) = thread["activeProviderThreadId"]
            .as_str()
            .and_then(|id| providers.get(id))
        else {
            report.skip("missing native session");
            continue;
        };
        let Some(native_id) = provider["nativeThreadRef"]["nativeId"]
            .as_str()
            .filter(|id| valid_native_id(id))
        else {
            report.skip("missing native session");
            continue;
        };
        let instance = required(provider, "providerInstanceId")?;
        let (kind, profile_id) = profiles.get(instance).ok_or_else(|| {
            format!("T3 instance {instance:?} requires --profile {instance}=DESTINATION")
        })?;
        if driver(required(provider, "driver")?)? != *kind {
            return Err(format!("provider driver mismatch for {instance:?}"));
        }
        let external = format!("t3code:{id}");
        let native_prefix = if *kind == ProviderKind::Codex {
            "codex"
        } else {
            "claude"
        };
        let cursor_key = if *kind == ProviderKind::Codex {
            "thread_id"
        } else {
            "session_id"
        };
        let converted: Result<PreparedThread, String> = (|| {
            let matches: Vec<_> = index
                .sessions
                .iter()
                .filter(|meta| {
                    meta.imported_from.as_deref() == Some(external.as_str())
                        || (meta.provider == *kind
                            && meta
                                .resume_cursor
                                .as_ref()
                                .is_some_and(|cursor| cursor.0[cursor_key] == native_id))
                })
                .collect();
            let existing = match matches.as_slice() {
                [] => None,
                [meta]
                    if meta.imported_from.as_deref() == Some(external.as_str())
                        || meta.imported_from.as_deref()
                            == Some(format!("{native_prefix}:{native_id}").as_str()) =>
                {
                    Some(*meta)
                }
                [_] => return Err(
                    "native session conflicts with a tcode-created or differently imported thread"
                        .into(),
                ),
                _ => return Err("ambiguous destination native session".into()),
            };
            let mut meta = SessionMeta::new(
                *kind,
                cwd,
                thread["modelSelection"]["model"]
                    .as_str()
                    .map(str::to_string),
            );
            if let Some(existing) = existing {
                if Path::new(&existing.id).components().count() != 1 || existing.id == ".." {
                    return Err("unsafe destination thread id".into());
                }
                meta.id.clone_from(&existing.id);
            }
            if !claimed.insert((*kind, native_id.to_string())) {
                return Err("multiple T3 threads share the same native session".into());
            }
            meta.profile_id = profile_id.clone();
            meta.project_id = Some(project.id.clone());
            meta.title = required(&thread, "title")?.into();
            meta.created_at = timestamp(required(&thread, "createdAt")?)? / 1000;
            meta.updated_at = timestamp(required(&thread, "updatedAt")?)? / 1000;
            meta.archived_at = thread["archivedAt"]
                .as_str()
                .map(timestamp)
                .transpose()?
                .map(|ts| ts / 1000);
            meta.settled_at = if settled {
                Some(
                    thread["settledAt"]
                        .as_str()
                        .map(timestamp)
                        .transpose()?
                        .map_or(meta.updated_at, |ts| ts / 1000),
                )
            } else {
                None
            };
            meta.imported_from = Some(external);
            meta.resume_cursor = Some(ResumeCursor(json!({cursor_key: native_id})));
            meta.interaction_mode = if thread["interactionMode"] == "plan" {
                InteractionMode::Plan
            } else {
                InteractionMode::Build
            };
            meta.approval_mode = if thread["runtimeMode"] == "full-access" {
                ApprovalMode::FullAccess
            } else {
                ApprovalMode::Supervised
            };
            if let Some(options) = thread["modelSelection"].get("options") {
                let selections: Vec<OptionSelection> = serde_json::from_value(options.clone())
                    .map_err(|e| format!("model options: {e}"))?;
                meta.option_selections = selections
                    .into_iter()
                    .filter(|option| match kind {
                        ProviderKind::Codex => {
                            matches!(option.id.as_str(), "reasoningEffort" | "serviceTier")
                        }
                        ProviderKind::ClaudeCode => matches!(
                            option.id.as_str(),
                            "reasoningEffort" | "contextWindow" | "fastMode" | "thinking"
                        ),
                        _ => false,
                    })
                    .collect();
            }
            let (bytes, attachments) = history::convert(&db, &thread, &meta, native_id)?;
            report.omitted_attachments += attachments;
            Ok(PreparedThread {
                meta,
                bytes,
                refresh: existing.is_some(),
            })
        })();
        match converted {
            Ok(thread) => prepared.push(thread),
            Err(error) => report.failures.push(format!("thread {id}: {error}")),
        }
    }
    // Keep the read transaction alive through discovery, then release T3 before writing.
    drop(query);
    db.commit().map_err(|e| e.to_string())?;
    if !report.failures.is_empty() {
        report.projects_created = 0;
        return Ok(report);
    }
    let _write_lock;
    let store = if options.dry_run {
        store
    } else {
        let store = SessionStore::open_at(options.data_dir.clone()).map_err(|e| e.to_string())?;
        _write_lock = if inspection_lock.is_none() {
            store.lock_exclusive(true).map_err(|e| e.to_string())?
        } else {
            None
        };
        let current = store.read_file_strict().map_err(|e| e.to_string())?;
        if current.projects != original_projects || current.sessions != original_sessions {
            return Err(
                "destination index changed during conversion; close tcode and retry".into(),
            );
        }
        store
            .persist_index(&index)
            .map_err(|e| format!("write projects: {e}"))?;
        store
    };
    for mut thread in prepared {
        if !options.dry_run {
            let result = (|| -> std::io::Result<()> {
                let old = store.read_event_log(&thread.meta.id)?;
                store.write_event_log(&thread.meta.id, &thread.bytes)?;
                thread.meta.last_user_message_at = None;
                store.recover_last_user_message_at(&mut thread.meta);
                let mut next = index.clone();
                next.sessions.retain(|meta| meta.id != thread.meta.id);
                next.sessions.push(thread.meta.clone());
                if let Err(error) = store.persist_index(&next) {
                    if let Err(rollback) = store.write_event_log(&thread.meta.id, &old) {
                        return Err(std::io::Error::other(format!(
                            "index replacement failed: {error}; log rollback failed: {rollback}"
                        )));
                    }
                    return Err(error);
                }
                index = next;
                Ok(())
            })();
            if let Err(error) = result {
                report
                    .failures
                    .push(format!("write thread {}: {error}", thread.meta.id));
                continue;
            }
        }
        if thread.refresh {
            report.threads_refreshed += 1;
        } else {
            report.threads_created += 1;
        }
    }
    Ok(report)
}

fn valid_native_id(id: &str) -> bool {
    // Both supported adapters resume UUID sessions; synthetic pending refs cannot resume.
    id.len() == 36
        && id.bytes().enumerate().all(|(i, byte)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
