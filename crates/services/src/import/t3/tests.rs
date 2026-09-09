use super::*;
use agent::{AgentEvent, ItemContent, TurnStatus};
use tcode_core::session::Timeline;

const NATIVE: &str = "01900000-0000-7000-8000-000000000001";
const DATE: &str = "2026-09-01T12:00:00.000Z";

struct Fixture {
    root: PathBuf,
    db: Connection,
    options: ImportOptions,
}

impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp")
            .join(format!("t3-import-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let db = Connection::open(source.join("state.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE orchestration_v2_projection_metadata (projection_name TEXT, schema_version INTEGER);
            INSERT INTO orchestration_v2_projection_metadata VALUES ('thread-projections', 2);
            CREATE TABLE projection_projects (project_id TEXT, title TEXT, workspace_root TEXT, created_at TEXT, deleted_at TEXT);").unwrap();
        for table in [
            "threads",
            "provider_threads",
            "runs",
            "messages",
            "turn_items",
        ] {
            db.execute_batch(&format!("CREATE TABLE orchestration_v2_projection_{table} (thread_id TEXT, payload_json TEXT);")).unwrap();
        }
        db.execute(
            "INSERT INTO projection_projects VALUES ('project', 'T3 project name', ?1, ?2, NULL)",
            rusqlite::params![root.to_str().unwrap(), DATE],
        )
        .unwrap();
        Self {
            options: ImportOptions {
                source,
                data_dir: root.join("destination"),
                ..ImportOptions::default()
            },
            root,
            db,
        }
    }

    fn insert(&self, table: &str, thread: &str, value: Value) {
        self.db
            .execute(
                &format!("INSERT INTO orchestration_v2_projection_{table} VALUES (?1, ?2)"),
                rusqlite::params![thread, value.to_string()],
            )
            .unwrap();
    }

    fn thread(&self, id: &str, instance: &str, native: &str) {
        let driver = if instance == "claudeAgent" {
            "claudeAgent"
        } else {
            "codex"
        };
        self.insert("threads", id, json!({"id": id, "projectId": "project", "title": format!("T3 {id}"), "createdAt": DATE, "updatedAt": "2026-09-02T12:00:00Z", "activeProviderThreadId": format!("provider-{id}"), "modelSelection": {"model": "chosen-model", "options": [{"id":"reasoningEffort", "value":"high"}]}, "runtimeMode":"full-access", "interactionMode":"build"}));
        self.insert("provider_threads", id, json!({"id":format!("provider-{id}"), "driver":driver, "providerInstanceId":instance, "nativeThreadRef":{"nativeId":native}}));
    }

    fn run(&self, thread: &str, id: &str, ordinal: u64, status: &str) {
        self.insert("runs", thread, json!({"id": id, "ordinal":ordinal, "requestedAt":DATE, "completedAt":DATE, "status":status}));
    }

    fn item(&self, thread: &str, id: &str, ordinal: u64, extra: Value) {
        let mut value = json!({"id": id, "runId":"run", "ordinal":ordinal, "status":"completed", "startedAt":DATE, "completedAt":DATE, "updatedAt":DATE});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        self.insert("turn_items", thread, value);
    }

    fn update(&self, table: &str, thread: &str, field: &str, value: Value) {
        self.db.execute(&format!("UPDATE orchestration_v2_projection_{table} SET payload_json = json_set(payload_json, ?1, json(?2)) WHERE thread_id = ?3"), rusqlite::params![format!("$.{field}"), value.to_string(), thread]).unwrap();
    }

    fn store(&self) -> SessionStore {
        SessionStore::inspect_at(self.options.data_dir.clone())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn imports_profiles_canonical_history_and_saved_status_then_refreshes_in_place() {
    let mut f = Fixture::new();
    f.thread("codex", "codex", NATIVE);
    f.thread(
        "claude",
        "claudeAgent",
        "01900000-0000-7000-8000-000000000002",
    );
    f.thread(
        "custom",
        "Openrouter",
        "01900000-0000-7000-8000-000000000003",
    );
    std::fs::write(
        f.options.source.join("settings.json"),
        r#"{"providerInstances":{"Openrouter":{"driver":"codex"}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(&f.options.data_dir).unwrap();
    std::fs::write(
        f.options.data_dir.join("settings.json"),
        r#"{"profiles":{"openrouter":{"kind":"codex"}}}"#,
    )
    .unwrap();
    f.options
        .profiles
        .insert("Openrouter".into(), "openrouter".into());
    f.update("threads", "custom", "settledOverride", json!("settled"));
    f.update("threads", "custom", "settledAt", json!(DATE));
    f.update("threads", "claude", "archivedAt", json!(DATE));
    let worktree = f.root.join("worktree");
    std::fs::create_dir(&worktree).unwrap();
    f.update("threads", "custom", "worktreePath", json!(worktree));
    // Insert run and item records out of order to exercise projection ordering.
    f.run("codex", "later", 2, "running");
    f.run("codex", "run", 1, "completed");
    f.item(
        "codex",
        "answer",
        8,
        json!({"type":"assistant_message", "messageId":"answer-message", "text":"answer"}),
    );
    f.item(
        "codex",
        "command",
        2,
        json!({"type":"command_execution", "input":"pwd", "output":"/project", "exitCode":0}),
    );
    f.item("codex", "prompt", 1, json!({"type":"user_message", "messageId":"message", "text":"stale item text", "attachments":[{"id":"image"}]}));
    f.item(
        "codex",
        "duplicate",
        3,
        json!({"type":"user_message", "messageId":"message", "text":"duplicate"}),
    );
    f.insert("messages", "codex", json!({"id":"message", "runId":"run", "role":"user", "text":"canonical prompt", "createdAt":DATE, "attachments":[{"id":"image"}]}));
    f.insert("messages", "codex", json!({"id":"unreferenced", "runId":"later", "role":"assistant", "text":"retained", "createdAt":DATE}));
    let store = SessionStore::open_at(f.options.data_dir.clone()).unwrap();
    let mut adopted = SessionMeta::new(ProviderKind::Codex, f.root.clone(), None);
    adopted.imported_from = Some(format!("codex:{NATIVE}"));
    adopted.resume_cursor = Some(ResumeCursor(json!({"thread_id":NATIVE})));
    store.upsert_meta(&adopted).unwrap();
    let unrelated = SessionMeta::new(ProviderKind::ClaudeCode, f.root.clone(), None);
    store.upsert_meta(&unrelated).unwrap();
    store
        .write_event_log(&unrelated.id, b"unchanged unrelated log\n")
        .unwrap();
    let report = import(&f.options).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(
        (
            report.threads_created,
            report.threads_refreshed,
            report.omitted_attachments
        ),
        (2, 1, 1)
    );
    let index = store.read_file_strict().unwrap();
    let codex = index
        .sessions
        .iter()
        .find(|meta| meta.id == adopted.id)
        .unwrap();
    assert_eq!(codex.title, "T3 codex");
    assert_eq!(codex.imported_from.as_deref(), Some("t3code:codex"));
    assert_eq!(codex.model.as_deref(), Some("chosen-model"));
    assert_eq!(codex.option_selections[0].value, "high");
    let custom = index
        .sessions
        .iter()
        .find(|meta| meta.title == "T3 custom")
        .unwrap();
    assert_eq!(custom.profile_id.as_deref(), Some("openrouter"));
    assert!(custom.settled_at.is_some());
    assert_eq!(custom.cwd, worktree);
    assert!(custom.worktree.is_none());
    let claude = index
        .sessions
        .iter()
        .find(|meta| meta.title == "T3 claude")
        .unwrap();
    assert!(claude.archived_at.is_some());
    assert!(
        claude
            .resume_cursor
            .as_ref()
            .unwrap()
            .0
            .get("session_id")
            .is_some()
    );
    assert_eq!(codex.project_id, custom.project_id);
    let events = store.read_events(&codex.id);
    let items: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            AgentEvent::ItemCompleted(item) => Some(item),
            _ => None,
        })
        .collect();
    assert_eq!(
        items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["prompt", "command", "answer", "unreferenced"]
    );
    assert!(
        matches!(&items[0].content, ItemContent::UserMessage { text, attachments, .. } if text == "canonical prompt" && attachments.is_empty())
    );
    assert!(events.iter().any(|event| matches!(&event.event, AgentEvent::TurnCompleted { turn_id, status:TurnStatus::Interrupted, .. } if turn_id == "later")));
    let ids: HashSet<_> = index.sessions.iter().map(|meta| meta.id.clone()).collect();
    store
        .write_event_log(&adopted.id, b"tcode-side edit\n")
        .unwrap();
    f.update("threads", "codex", "title", json!("Refreshed title"));
    let report = import(&f.options).unwrap();
    assert_eq!((report.threads_created, report.threads_refreshed), (0, 3));
    let refreshed = store.read_file_strict().unwrap();
    assert_eq!(
        refreshed
            .sessions
            .iter()
            .map(|meta| meta.id.clone())
            .collect::<HashSet<_>>(),
        ids
    );
    assert_eq!(
        refreshed
            .sessions
            .iter()
            .find(|meta| meta.id == adopted.id)
            .unwrap()
            .title,
        "Refreshed title"
    );
    assert_eq!(
        store.read_event_log(&unrelated.id).unwrap(),
        b"unchanged unrelated log\n"
    );
    assert!(refreshed.sessions.contains(&unrelated));
    assert_eq!(store.read_events(&adopted.id), events);
}

#[test]
fn converts_activity_without_restoring_interactive_work() {
    let f = Fixture::new();
    f.thread("thread", "codex", NATIVE);
    f.run("thread", "run", 1, "waiting");
    for (i, value) in [
        json!({"type":"dynamic_tool", "toolName":"tool", "input":{"a":1}, "output":{"result":"ok"}}),
        json!({"type":"file_change", "fileName":"a.rs", "diffStr":"@@ -1 +1 @@\n-old\n+new\n"}),
        json!({"type":"web_search", "patterns":["query"]}),
        json!({"type":"subagent", "title":"review", "prompt":"inspect", "result":"done"}),
        json!({"type":"todo_list", "steps":[{"text":"test", "status":"completed"}, {"text":"follow-up", "status":"running"}]}),
        json!({"type":"proposed_plan", "markdown":"# Plan"}),
        json!({"type":"compaction", "beforeTokenCount":100}),
        json!({"type":"approval_request", "prompt":"command", "status":"waiting"}),
        json!({"type":"user_input_request", "questions":[{"question":"why?"}], "status":"waiting"}),
        json!({"type":"checkpoint", "checkpointId":"native"}),
    ].into_iter().enumerate() { f.item("thread", &format!("item-{i}"), i as u64, value); }
    let report = import(&f.options).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    let store = f.store();
    let events = store.read_events(&store.load_index()[0].id);
    let mut timeline = Timeline::default();
    for event in &events {
        timeline.apply_at(event.ts, &event.event);
    }
    assert!(!timeline.turn_running);
    assert!(timeline.pending_approvals.is_empty());
    assert!(timeline.pending_user_input.is_none());
    assert!(timeline.plan_ready().is_none());
    assert_eq!(
        timeline.plan_steps[1].status,
        agent::PlanStepStatus::InProgress
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e.event, AgentEvent::PlanUpdated { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e.event, AgentEvent::ContextCompacted(_)))
    );
    let kinds: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.event {
            AgentEvent::ItemCompleted(item) => Some(&item.content),
            _ => None,
        })
        .collect();
    assert!(matches!(kinds[0], ItemContent::ToolCall { .. }));
    assert!(
        matches!(kinds[1], ItemContent::FileChange { changes, .. } if changes[0].diff.is_some())
    );
    assert!(matches!(kinds[2], ItemContent::WebSearch { query } if query == "query"));
    assert!(
        matches!(kinds[3], ItemContent::Subagent { summary:Some(summary), .. } if summary == "done")
    );
    assert!(
        kinds[4..]
            .iter()
            .all(|kind| matches!(kind, ItemContent::Other { .. }))
    );
}

#[test]
fn filters_are_independent_and_dry_run_and_failures_leave_files_untouched() {
    let mut f = Fixture::new();
    f.thread("settled", "codex", NATIVE);
    f.thread("archived", "codex", "01900000-0000-7000-8000-000000000002");
    f.thread(
        "missing-worktree",
        "codex",
        "01900000-0000-7000-8000-000000000003",
    );
    f.thread("missing-session", "codex", "pending:run");
    f.thread("deleted", "codex", "01900000-0000-7000-8000-000000000004");
    f.update("threads", "settled", "settledOverride", json!("settled"));
    f.update("threads", "archived", "archivedAt", json!(DATE));
    f.update(
        "threads",
        "missing-worktree",
        "worktreePath",
        json!(f.root.join("missing")),
    );
    f.update("threads", "deleted", "deletedAt", json!(DATE));
    f.options.dry_run = true;
    let source = std::fs::read(f.options.source.join("state.sqlite")).unwrap();
    let report = import(&f.options).unwrap();
    assert_eq!(report.threads_created, 2);
    assert_eq!(report.exclusions["missing thread directory"], 1);
    assert_eq!(report.exclusions["missing native session"], 1);
    assert_eq!(report.exclusions["deleted thread"], 1);
    assert!(!f.options.data_dir.exists());
    assert_eq!(
        std::fs::read(f.options.source.join("state.sqlite")).unwrap(),
        source
    );
    f.options.skip_settled = true;
    assert_eq!(import(&f.options).unwrap().threads_created, 1);
    f.options.skip_settled = false;
    f.options.skip_archived = true;
    assert_eq!(import(&f.options).unwrap().threads_created, 1);
    f.options.skip_settled = true;
    f.options.dry_run = false;
    assert_eq!(import(&f.options).unwrap().threads_created, 0);
    assert_eq!(
        f.store().read_file_strict().unwrap().projects[0].name,
        "T3 project name"
    );
    f.options.skip_settled = false;
    f.options.skip_archived = false;
    import(&f.options).unwrap();
    let before = snapshot(&f.options.data_dir);
    f.options.dry_run = true;
    import(&f.options).unwrap();
    assert_eq!(snapshot(&f.options.data_dir), before);
    f.options.dry_run = false;
    f.run("settled", "run", 1, "completed");
    f.item(
        "settled",
        "bad",
        1,
        json!({"type":"command_execution", "input":42}),
    );
    assert_eq!(import(&f.options).unwrap().failures.len(), 1);
    assert_eq!(snapshot(&f.options.data_dir), before);
    f.db.execute(
        "UPDATE orchestration_v2_projection_metadata SET schema_version=999",
        [],
    )
    .unwrap();
    assert!(import(&f.options).unwrap_err().contains("unsupported T3"));
    assert_eq!(snapshot(&f.options.data_dir), before);
    f.db.execute(
        "UPDATE orchestration_v2_projection_metadata SET schema_version=2",
        [],
    )
    .unwrap();
    f.options.profiles.insert("codex".into(), "claude".into());
    assert!(import(&f.options).unwrap_err().contains("incompatible"));
    assert_eq!(snapshot(&f.options.data_dir), before);
    f.options.profiles.clear();
    std::fs::write(f.options.data_dir.join("sessions.json"), b"broken index").unwrap();
    let before = snapshot(&f.options.data_dir);
    assert!(
        import(&f.options)
            .unwrap_err()
            .contains("destination index")
    );
    assert_eq!(snapshot(&f.options.data_dir), before);
}

fn snapshot(path: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_str().unwrap().into(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn native_conflicts_missing_projects_and_busy_destination_are_reported_without_writes() {
    let mut f = Fixture::new();
    f.thread("thread", "codex", NATIVE);
    f.db.execute(
        "INSERT INTO projection_projects VALUES ('missing', 'Missing', ?1, ?2, NULL)",
        rusqlite::params![f.root.join("missing").to_str().unwrap(), DATE],
    )
    .unwrap();
    f.db.execute(
        "INSERT INTO projection_projects VALUES ('deleted', 'Deleted', ?1, ?2, ?2)",
        rusqlite::params![f.root.to_str().unwrap(), DATE],
    )
    .unwrap();
    let store = SessionStore::open_at(f.options.data_dir.clone()).unwrap();
    let mut native = SessionMeta::new(ProviderKind::Codex, f.root.clone(), None);
    native.resume_cursor = Some(ResumeCursor(json!({"thread_id":NATIVE})));
    store.upsert_meta(&native).unwrap();
    store
        .write_event_log(&native.id, b"native tcode history\n")
        .unwrap();
    let before = snapshot(&f.options.data_dir);
    let report = import(&f.options).unwrap();
    assert!(report.failures[0].contains("conflicts"));
    assert_eq!(report.exclusions["missing project directory"], 1);
    assert_eq!(report.exclusions["deleted project"], 1);
    assert_eq!(snapshot(&f.options.data_dir), before);
    native.imported_from = Some(format!("codex:{NATIVE}"));
    store.upsert_meta(&native).unwrap();
    let mut duplicate = native.clone();
    duplicate.id = "second-import".into();
    store.upsert_meta(&duplicate).unwrap();
    let before = snapshot(&f.options.data_dir);
    assert!(import(&f.options).unwrap().failures[0].contains("ambiguous"));
    assert_eq!(snapshot(&f.options.data_dir), before);
    let _lock = store.lock_exclusive(true).unwrap();
    let before = snapshot(&f.options.data_dir);
    for dry_run in [false, true] {
        f.options.dry_run = dry_run;
        assert!(
            import(&f.options)
                .unwrap_err()
                .contains("close the destination")
        );
        assert_eq!(snapshot(&f.options.data_dir), before);
    }
}

#[test]
fn failed_log_replacement_preserves_the_previous_import() {
    let f = Fixture::new();
    f.thread("thread", "codex", NATIVE);
    import(&f.options).unwrap();
    let store = f.store();
    let meta = store.load_index().remove(0);
    let previous_log = store.read_event_log(&meta.id).unwrap();
    f.update("threads", "thread", "title", json!("new title"));
    // A leftover directory or filesystem obstruction must be a write failure,
    // never a skip or a partial replacement of an existing conversation.
    std::fs::create_dir(f.options.data_dir.join(format!("{}.jsonl.tmp", meta.id))).unwrap();
    let report = import(&f.options).unwrap();
    assert_eq!(report.threads_refreshed, 0);
    assert!(report.failures[0].starts_with("write thread"));
    assert_eq!(store.read_event_log(&meta.id).unwrap(), previous_log);
    assert_eq!(store.load_index()[0], meta);
}

#[test]
fn migrated_unassociated_history_keeps_messages_and_completed_turns() {
    let f = Fixture::new();
    f.thread("migrated", "codex", NATIVE);
    f.item("migrated", "answer", 2, json!({"type":"assistant_message", "messageId":"answer-message", "text":"done", "runId":null}));
    f.item("migrated", "prompt", 1, json!({"type":"user_message", "messageId":"prompt-message", "text":"question", "runId":null}));
    let report = import(&f.options).unwrap();
    assert!(report.failures.is_empty());
    let store = f.store();
    let timeline = Timeline::fold_events(store.read_events(&store.load_index()[0].id));
    assert_eq!(timeline.entries.len(), 2);
    assert_eq!(timeline.entries[0].id, "prompt");
    assert_eq!(timeline.entries[1].id, "answer");
    assert_eq!(timeline.turns[0].status, Some(TurnStatus::Completed));
}

#[test]
fn recent_project_detection_and_live_import_are_scoped_and_preserve_known_sessions() {
    let f = Fixture::new();
    f.thread("codex", "codex", NATIVE);
    f.thread(
        "custom",
        "Openrouter",
        "01900000-0000-7000-8000-000000000002",
    );
    f.update("threads", "custom", "settledOverride", json!("settled"));
    f.update("threads", "custom", "archivedAt", json!(DATE));
    std::fs::write(
        f.options.source.join("settings.json"),
        r#"{"providerInstances":{"Openrouter":{"driver":"codex"}}}"#,
    )
    .unwrap();
    let other = f.root.join("other-project");
    std::fs::create_dir(&other).unwrap();
    f.db.execute(
        "INSERT INTO projection_projects VALUES ('other', 'Other project', ?1, ?2, NULL)",
        rusqlite::params![other.to_str().unwrap(), DATE],
    )
    .unwrap();
    f.thread("other", "Unmapped", "01900000-0000-7000-8000-000000000003");
    f.update("threads", "other", "projectId", json!("other"));

    let detected = inspect_project(&f.options.source, &f.root.join("."))
        .unwrap()
        .unwrap();
    assert_eq!(detected.title, "T3 project name");
    assert_eq!(
        detected.profiles,
        vec![tcode_protocol::T3ImportProfile {
            id: "Openrouter".into(),
            provider: ProviderKind::Codex
        }]
    );
    assert!(
        inspect_project(&f.options.source, &f.options.source)
            .unwrap()
            .is_none()
    );
    assert!(
        inspect_project(&f.root.join("missing"), &f.root)
            .unwrap()
            .is_none()
    );
    assert!(
        !f.options.data_dir.exists(),
        "discovery must not create a destination"
    );

    let settings: Settings =
        serde_json::from_value(json!({"profiles":{"openrouter":{"kind":"codex"}}})).unwrap();
    let mappings = BTreeMap::from([("Openrouter".into(), "openrouter".into())]);
    let project = Project::from_root(f.root.clone());
    assert!(
        prepare_project(
            f.options.source.clone(),
            project.clone(),
            &settings,
            BTreeMap::new()
        )
        .is_err()
    );
    let (threads, report) = prepare_project(
        f.options.source.clone(),
        project.clone(),
        &settings,
        mappings.clone(),
    )
    .unwrap();
    assert_eq!(threads.len(), 2);
    assert!(
        report.exclusions.is_empty(),
        "other projects do not contribute to this import"
    );
    assert!(
        !f.options.data_dir.exists(),
        "conversion must not write destination files"
    );

    let store = SessionStore::open_at(f.options.data_dir.clone()).unwrap();
    let _host_lock = store.lock_exclusive(true).unwrap();
    store.upsert_project(&project).unwrap();
    // This native session arrived after conversion. The writer must preserve it.
    let mut native = SessionMeta::new(ProviderKind::Codex, f.root.clone(), None);
    native.resume_cursor = Some(ResumeCursor(json!({"thread_id":NATIVE})));
    native.title = "Live thread".into();
    store.upsert_meta(&native).unwrap();
    store.write_event_log(&native.id, b"live log\n").unwrap();
    let (metas, skipped) = write_new_threads(&store, threads).unwrap();
    assert_eq!(skipped, 1);
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].title, "T3 custom");
    assert_eq!(metas[0].profile_id.as_deref(), Some("openrouter"));
    assert_eq!(metas[0].project_id.as_deref(), Some(project.id.as_str()));
    assert!(metas[0].settled_at.is_some() && metas[0].archived_at.is_some());
    assert_eq!(store.read_event_log(&native.id).unwrap(), b"live log\n");
    let before = std::fs::read(store.root().join("sessions.json")).unwrap();
    let (again, _) =
        prepare_project(f.options.source.clone(), project, &settings, mappings).unwrap();
    let (metas, skipped) = write_new_threads(&store, again).unwrap();
    assert!(metas.is_empty());
    assert_eq!(skipped, 2);
    assert_eq!(
        std::fs::read(store.root().join("sessions.json")).unwrap(),
        before
    );

    f.db.execute(
        "UPDATE orchestration_v2_projection_metadata SET schema_version = 99",
        [],
    )
    .unwrap();
    assert!(
        inspect_project(&f.options.source, &f.root)
            .unwrap_err()
            .contains("unsupported")
    );
}

#[test]
fn recent_rows_count_t3_once_and_retain_the_complete_native_fallback() {
    use crate::import::{ExternalRoots, SourceTool, scan_recent_dirs};
    let f = Fixture::new();
    let claude_id = "01900000-0000-7000-8000-000000000002";
    f.thread("codex", "codex", NATIVE);
    f.thread("claude", "claudeAgent", claude_id);
    f.thread(
        "t3-only",
        "Openrouter",
        "01900000-0000-7000-8000-000000000003",
    );
    f.update("threads", "codex", "archivedAt", json!(DATE));
    f.update("threads", "t3-only", "settledOverride", json!("settled"));
    f.thread("deleted", "codex", "01900000-0000-7000-8000-000000000004");
    f.update("threads", "deleted", "deletedAt", json!(DATE));
    f.thread("subagent", "codex", "01900000-0000-7000-8000-000000000005");
    f.update(
        "threads",
        "subagent",
        "lineage",
        json!({"relationshipToParent":"subagent"}),
    );
    f.thread("missing-session", "codex", "pending");
    f.thread(
        "missing-worktree",
        "codex",
        "01900000-0000-7000-8000-000000000006",
    );
    f.update(
        "threads",
        "missing-worktree",
        "worktreePath",
        json!(f.root.join("missing")),
    );
    let roots = ExternalRoots {
        claude_projects: f.root.join("claude"),
        claude_desktop_meta: f.root.join("desktop"),
        codex_session_roots: vec![f.root.join("codex")],
    };
    std::fs::create_dir_all(roots.claude_projects.join("project")).unwrap();
    std::fs::create_dir_all(&roots.codex_session_roots[0]).unwrap();
    // Both native Codex and Claude representations overlap T3. A second file
    // for the same Codex session must not inflate the recent row's counts.
    for (file, id) in [
        ("shared", NATIVE),
        ("shared-copy", NATIVE),
        ("native-only", "native-only"),
    ] {
        std::fs::write(roots.codex_session_roots[0].join(format!("{file}.jsonl")), json!({"type":"session_meta", "payload":{"id":id, "cwd":f.root.join("."), "originator":"codex_exec"}}).to_string()).unwrap();
    }
    std::fs::write(roots.claude_projects.join("project/shared.jsonl"), json!({"sessionId":claude_id,"cwd":f.root.join("."),"type":"user","message":{"content":"question"}}).to_string()).unwrap();
    let mut recent = scan_recent_dirs(&roots, &[]);
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].threads.len(), 4);
    let fallback = recent[0].threads.clone();
    annotate_recent_dirs(&f.options.source, &mut recent, &HashSet::new()).unwrap();
    assert_eq!(
        recent[0].source_counts,
        HashMap::from([(SourceTool::T3Code, 3), (SourceTool::CodexCli, 1)])
    );
    assert_eq!(
        recent[0].threads, fallback,
        "declining T3 must still offer all native files"
    );

    let known = HashSet::from([format!("codex:{NATIVE}"), "t3code:t3-only".into()]);
    annotate_recent_dirs(&f.options.source, &mut recent, &known).unwrap();
    assert_eq!(
        recent[0].source_counts,
        HashMap::from([(SourceTool::T3Code, 1), (SourceTool::CodexCli, 1)])
    );
    assert!(
        !f.options.data_dir.exists(),
        "counting must not write destination data"
    );

    // A failed T3 read leaves the native scan intact for the host to return
    // with a visible warning, rather than quietly claiming these are Codex.
    f.db.execute(
        "UPDATE orchestration_v2_projection_metadata SET schema_version = 99",
        [],
    )
    .unwrap();
    let mut fallback_scan = scan_recent_dirs(&roots, &[]);
    assert!(annotate_recent_dirs(&f.options.source, &mut fallback_scan, &HashSet::new()).is_err());
    assert_eq!(fallback_scan[0].threads, fallback);
    assert!(fallback_scan[0].source_counts.is_empty());
}
