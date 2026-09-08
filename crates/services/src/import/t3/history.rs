use std::collections::{BTreeMap, HashMap, HashSet};

use agent::{
    AgentEvent, Compaction, FileChange, FileChangeKind, ItemContent, ItemStatus, PlanResolution,
    PlanStep, PlanStepStatus, ThreadItem, TurnStatus,
};
use rusqlite::Connection;
use serde_json::Value;
use tcode_core::project::SessionMeta;
use tcode_core::session::StoredEvent;

use super::{payloads, required, timestamp};

struct Record {
    value: Value,
    message: bool,
    ts: u64,
}

fn time(value: &Value) -> Result<u64, String> {
    let text = value["startedAt"]
        .as_str()
        .or(value["createdAt"].as_str())
        .or(value["updatedAt"].as_str())
        .ok_or("history record has no timestamp")?;
    timestamp(text)
}

pub(super) fn convert(
    db: &Connection,
    thread: &Value,
    meta: &SessionMeta,
    native_id: &str,
) -> Result<(Vec<u8>, usize), String> {
    let id = required(thread, "id")?;
    let mut runs = payloads(db, "runs", Some(id))?;
    runs.sort_by_key(|run| run["ordinal"].as_u64());
    let run_order: HashMap<_, _> = runs
        .iter()
        .map(|run| {
            Ok((
                required(run, "id")?,
                run["ordinal"].as_u64().ok_or("invalid run ordinal")?,
            ))
        })
        .collect::<Result<_, String>>()?;
    let mut items = payloads(db, "turn_items", Some(id))?;
    items.sort_by_key(|item| item["ordinal"].as_u64());
    let messages = payloads(db, "messages", Some(id))?;
    let messages: HashMap<_, _> = messages
        .into_iter()
        .map(|message| Ok((required(&message, "id")?.to_string(), message)))
        .collect::<Result<_, String>>()?;
    let mut used_messages = HashSet::new();
    let mut attachment_ids = HashSet::new();
    let mut groups: BTreeMap<(u64, u8), Vec<Record>> = BTreeMap::new();
    let group_key = |value: &Value, ts: u64| -> Result<(u64, u8), String> {
        if let Some(run) = value["runId"].as_str() {
            return Ok((
                *run_order
                    .get(run)
                    .ok_or_else(|| format!("history references missing run {run}"))?,
                1,
            ));
        }
        // Unassociated migrated history sits between runs by its recorded time.
        for run in &runs {
            if timestamp(required(run, "requestedAt")?)? > ts {
                return Ok((run["ordinal"].as_u64().unwrap(), 0));
            }
        }
        Ok((u64::MAX, 0))
    };
    for mut item in items {
        item["ordinal"]
            .as_u64()
            .ok_or("invalid turn-item ordinal")?;
        let ts = time(&item)?;
        let key = group_key(&item, ts)?;
        let is_message = matches!(
            item["type"].as_str(),
            Some("user_message" | "assistant_message")
        );
        if is_message {
            let message_id = required(&item, "messageId")?.to_string();
            if !used_messages.insert(message_id.clone()) {
                continue;
            }
            if let Some(message) = messages.get(&message_id) {
                // Canonical messages own text; turn items own position and parent activity.
                item["text"] = message["text"].clone();
                count_attachments(message, &message_id, &mut attachment_ids);
            } else {
                count_attachments(&item, &message_id, &mut attachment_ids);
            }
        }
        groups.entry(key).or_default().push(Record {
            value: item,
            message: false,
            ts,
        });
    }
    let mut remaining: Vec<_> = messages
        .into_iter()
        .filter(|(id, _)| !used_messages.contains(id))
        .collect();
    remaining.sort_by(|(a_id, a), (b_id, b)| {
        a["createdAt"]
            .as_str()
            .cmp(&b["createdAt"].as_str())
            .then(a_id.cmp(b_id))
    });
    for (id, message) in remaining {
        let ts = time(&message)?;
        let records = groups.entry(group_key(&message, ts)?).or_default();
        count_attachments(&message, &id, &mut attachment_ids);
        let position = records
            .iter()
            .position(|record| record.ts > ts)
            .unwrap_or(records.len());
        records.insert(
            position,
            Record {
                value: message,
                message: true,
                ts,
            },
        );
    }
    // Empty and unfinished runs are history too, but never become live work.
    for ordinal in run_order.values() {
        groups.entry((*ordinal, 1)).or_default();
    }
    let mut events = vec![StoredEvent {
        ts: Some(timestamp(required(thread, "createdAt")?)?),
        event: AgentEvent::SessionStarted {
            provider_session_id: native_id.into(),
            resume: meta.resume_cursor.clone().unwrap(),
            model: meta.model.clone(),
        },
    }];
    for ((ordinal, phase), records) in groups {
        let run = runs
            .iter()
            .find(|run| phase == 1 && run["ordinal"].as_u64() == Some(ordinal));
        if let Some(run) = run {
            let turn_id = required(run, "id")?.to_string();
            push(
                &mut events,
                timestamp(required(run, "requestedAt")?)?,
                AgentEvent::TurnStarted {
                    turn_id: turn_id.clone(),
                },
            );
            for record in records {
                append(&mut events, record)?;
            }
            let status = match required(run, "status")? {
                "completed" => TurnStatus::Completed,
                "failed" => TurnStatus::Failed,
                _ => TurnStatus::Interrupted,
            };
            let ts = run["completedAt"]
                .as_str()
                .map(timestamp)
                .transpose()?
                .unwrap_or_else(|| events.last().and_then(|event| event.ts).unwrap());
            push(
                &mut events,
                ts,
                AgentEvent::TurnCompleted {
                    turn_id,
                    status,
                    usage: None,
                },
            );
        } else {
            let mut open = None;
            let mut finished = false;
            for record in records {
                let user = record.value["parentItemId"].is_null()
                    && (record.value["type"] == "user_message"
                        || (record.message && record.value["role"] == "user"));
                if user || open.is_none() {
                    if let Some(turn_id) = open.take() {
                        push(
                            &mut events,
                            record.ts,
                            AgentEvent::TurnCompleted {
                                turn_id,
                                status: if finished {
                                    TurnStatus::Completed
                                } else {
                                    TurnStatus::Interrupted
                                },
                                usage: None,
                            },
                        );
                    }
                    finished = false;
                    let turn_id = format!("t3-history:{}", required(&record.value, "id")?);
                    push(
                        &mut events,
                        record.ts,
                        AgentEvent::TurnStarted {
                            turn_id: turn_id.clone(),
                        },
                    );
                    open = Some(turn_id);
                }
                if record.value["type"] == "assistant_message"
                    || (record.message && record.value["role"] == "assistant")
                {
                    finished = record.value["streaming"] != true
                        && (record.message || record.value["status"] == "completed");
                }
                append(&mut events, record)?;
            }
            if let Some(turn_id) = open {
                let ts = events.last().and_then(|event| event.ts).unwrap();
                push(
                    &mut events,
                    ts,
                    AgentEvent::TurnCompleted {
                        turn_id,
                        status: if finished {
                            TurnStatus::Completed
                        } else {
                            TurnStatus::Interrupted
                        },
                        usage: None,
                    },
                );
            }
        }
    }
    let mut bytes = Vec::new();
    for event in events {
        serde_json::to_writer(&mut bytes, &event).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
    }
    Ok((bytes, attachment_ids.len()))
}

fn count_attachments(value: &Value, message_id: &str, ids: &mut HashSet<String>) {
    if let Some(attachments) = value["attachments"].as_array() {
        for (index, _) in attachments.iter().enumerate() {
            ids.insert(format!("{message_id}:{index}"));
        }
    }
}

fn push(events: &mut Vec<StoredEvent>, ts: u64, event: AgentEvent) {
    events.push(StoredEvent {
        ts: Some(ts),
        event,
    });
}

fn item_status(value: &Value) -> ItemStatus {
    match value["status"].as_str() {
        Some("completed") => ItemStatus::Completed,
        Some("failed") => ItemStatus::Failed,
        _ => ItemStatus::Interrupted,
    }
}

fn display(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn append(events: &mut Vec<StoredEvent>, record: Record) -> Result<(), String> {
    let value = record.value;
    let id = required(&value, "id")?.to_string();
    let kind = if record.message {
        required(&value, "role")?
    } else {
        required(&value, "type")?
    };
    let status = item_status(&value);
    let content = match kind {
        "user" | "user_message" if value["parentItemId"].is_null() => ItemContent::UserMessage {
            text: required(&value, "text")?.into(),
            context_len: None,
            attachments: Vec::new(),
        },
        "user" | "user_message" => ItemContent::Other {
            provider_kind: "t3-subagent-input".into(),
            summary: required(&value, "text")?.into(),
        },
        "assistant" | "assistant_message" => ItemContent::AssistantMessage {
            text: required(&value, "text")?.into(),
        },
        "reasoning" => ItemContent::Reasoning {
            text: required(&value, "text")?.into(),
        },
        "command_execution" => ItemContent::CommandExecution {
            command: required(&value, "input")?.into(),
            output: value["output"].as_str().unwrap_or_default().into(),
            exit_code: value["exitCode"]
                .as_i64()
                .map(i32::try_from)
                .transpose()
                .map_err(|e| e.to_string())?,
            status,
        },
        "dynamic_tool" => ItemContent::ToolCall {
            name: value["toolName"].as_str().unwrap_or("T3 tool").into(),
            input: value["input"].clone(),
            output: value.get("output").map(display),
            status,
        },
        "file_change" => {
            let diff = value["diffStr"].as_str().map(str::to_string).or_else(|| {
                let old = value["oldStr"].as_str()?;
                let new = value["newStr"].as_str()?;
                Some(format!(
                    "@@ -1,{} +1,{} @@\n{}{}",
                    old.lines().count(),
                    new.lines().count(),
                    old.lines()
                        .map(|line| format!("-{line}\n"))
                        .collect::<String>(),
                    new.lines()
                        .map(|line| format!("+{line}\n"))
                        .collect::<String>()
                ))
            });
            ItemContent::FileChange {
                changes: vec![FileChange {
                    path: required(&value, "fileName")?.into(),
                    kind: FileChangeKind::Modify,
                    diff,
                }],
                status,
            }
        }
        "web_search" => ItemContent::WebSearch {
            query: value["patterns"]
                .as_array()
                .map(|patterns| patterns.iter().map(display).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default(),
        },
        "subagent" => ItemContent::Subagent {
            agent_type: value["title"].as_str().unwrap_or("T3 subagent").into(),
            description: required(&value, "prompt")?.into(),
            status,
            summary: value["result"]
                .as_str()
                .or(value["progress"].as_str())
                .map(str::to_string),
            model: None,
            effort: None,
        },
        "todo_list" => {
            let steps = value["steps"]
                .as_array()
                .ok_or("invalid task list")?
                .iter()
                .map(|step| {
                    Ok(PlanStep {
                        step: required(step, "text")?.into(),
                        status: match required(step, "status")? {
                            "completed" => PlanStepStatus::Completed,
                            "running" => PlanStepStatus::InProgress,
                            _ => PlanStepStatus::Pending,
                        },
                    })
                })
                .collect::<Result<_, String>>()?;
            push(
                events,
                record.ts,
                AgentEvent::PlanUpdated {
                    turn_id: value["runId"].as_str().map(str::to_string),
                    explanation: value["explanation"].as_str().map(str::to_string),
                    steps,
                },
            );
            return Ok(());
        }
        "proposed_plan" => {
            push(
                events,
                record.ts,
                AgentEvent::ProposedPlan {
                    item_id: id.clone(),
                    markdown: required(&value, "markdown")?.into(),
                },
            );
            push(
                events,
                record.ts,
                AgentEvent::PlanResolved {
                    item_id: id,
                    resolution: PlanResolution::Dismissed,
                },
            );
            return Ok(());
        }
        "compaction" => {
            push(
                events,
                record.ts,
                AgentEvent::ContextCompacted(Compaction {
                    pre_tokens: value["beforeTokenCount"].as_u64(),
                    post_tokens: value["afterTokenCount"].as_u64(),
                    ..Compaction::default()
                }),
            );
            if value["summary"].is_null() {
                return Ok(());
            }
            ItemContent::Other {
                provider_kind: "t3-compaction-summary".into(),
                summary: required(&value, "summary")?.into(),
            }
        }
        _ => {
            // Historical requests/checkpoints remain readable but cannot request input
            // or trigger a native operation during replay.
            let mut summary = value.clone();
            if let Some(object) = summary.as_object_mut() {
                object.remove("attachments");
            }
            ItemContent::Other {
                provider_kind: format!("t3-{kind}"),
                summary: serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?,
            }
        }
    };
    let item = ThreadItem {
        id,
        parent_item_id: value["parentItemId"].as_str().map(str::to_string),
        content,
    };
    if !record.message
        && matches!(
            kind,
            "command_execution" | "dynamic_tool" | "file_change" | "subagent"
        )
    {
        push(events, record.ts, AgentEvent::ItemStarted(item.clone()));
    }
    let completed = value["completedAt"]
        .as_str()
        .or(value["updatedAt"].as_str())
        .map(timestamp)
        .transpose()?
        .unwrap_or(record.ts);
    push(events, completed, AgentEvent::ItemCompleted(item));
    if kind == "web_search"
        && value["results"]
            .as_array()
            .is_some_and(|results| !results.is_empty())
    {
        push(
            events,
            completed,
            AgentEvent::ItemCompleted(ThreadItem {
                id: format!("{}:results", required(&value, "id")?),
                parent_item_id: value["parentItemId"].as_str().map(str::to_string),
                content: ItemContent::Other {
                    provider_kind: "t3-web-search-results".into(),
                    summary: serde_json::to_string_pretty(&value["results"])
                        .map_err(|e| e.to_string())?,
                },
            }),
        );
    }
    Ok(())
}
