use super::*;
use tcode_protocol::ThreadAttentionKind;

impl AppState {
    // Only the live provider path calls this; hydration and internal title work
    // never arm candidates or request attention. Read before folding the event,
    // since completion clears both pending input and interruption state.
    pub(super) fn observe_attention_event(&mut self, id: &str, event: &AgentEvent) {
        let Some(session) = self.resident(id) else {
            return;
        };
        let kind = match event {
            AgentEvent::ApprovalRequested(request)
                if !self
                    .approval_requests(id)
                    .iter()
                    .any(|pending| pending.id == request.id) =>
            {
                Some(ThreadAttentionKind::Approval)
            }
            AgentEvent::UserInputRequested { request_id, .. }
                if session
                    .timeline
                    .pending_user_input
                    .as_ref()
                    .is_none_or(|pending| pending.request_id != *request_id) =>
            {
                Some(ThreadAttentionKind::Question)
            }
            _ => None,
        };
        if let Some(kind) = kind {
            self.attention_requests.push((id.to_owned(), kind));
        }
        let session = self.resident_mut(id).unwrap();
        match event {
            AgentEvent::TurnCompleted { status, .. } => {
                let newly_completed = session
                    .timeline
                    .turns
                    .last()
                    .is_none_or(|turn| turn.status.is_none());
                if *status != TurnStatus::Completed || session.interrupt_requested {
                    session.completion_candidate = false;
                } else if newly_completed && session.meta.parent_session_id.is_none() {
                    session.completion_candidate = true;
                }
            }
            AgentEvent::TurnStarted { .. }
            | AgentEvent::SessionClosed { .. }
            | AgentEvent::Error { .. }
            | AgentEvent::ProviderStartFailed { .. } => {
                session.completion_candidate = false;
            }
            _ => {}
        }
    }

    fn attention_work_remaining(&self, id: &str) -> bool {
        self.pending_child_callbacks.contains_key(id)
            || !self.approval_requests(id).is_empty()
            || self.resident(id).is_some_and(|session| {
                session.has_work() || session.timeline.pending_user_input.is_some()
            })
            || self
                .sessions
                .iter()
                .filter(|meta| meta.parent_session_id.as_deref() == Some(id))
                .any(|child| self.attention_work_remaining(&child.id))
    }

    /// Called after the mailbox operation and its domain updates settle, including
    /// queue dispatch and child callbacks. These events have no snapshot or log.
    pub(crate) fn reconcile_attention(&mut self, cx: &mut HostCx) {
        let completed: Vec<_> = self
            .residents
            .live
            .values()
            .chain(self.residents.parked.values())
            .filter(|session| {
                session.completion_candidate && !self.attention_work_remaining(&session.meta.id)
            })
            .map(|session| session.meta.id.clone())
            .collect();
        for id in completed {
            self.resident_mut(&id).unwrap().completion_candidate = false;
            self.attention_requests
                .push((id, ThreadAttentionKind::Completed));
        }
        for (id, kind) in std::mem::take(&mut self.attention_requests) {
            if let Some(meta) = self.sessions.iter().find(|meta| meta.id == id) {
                emit_runtime(
                    cx,
                    RuntimeEvent::ThreadAttention {
                        session_id: id,
                        title: meta.title.clone(),
                        kind,
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{TestAppContext, TestClientState, TestStore};
    use super::*;
    use tcode_protocol::HostMessage;

    fn install(
        state: &mut AppState,
        id: &str,
        parent: Option<&str>,
    ) -> smol::channel::Receiver<SessionCommand> {
        let mut meta = SessionMeta::new(ProviderKind::Codex, state.store.root().to_owned(), None);
        meta.id = id.into();
        meta.title = format!("Title {id}");
        meta.parent_session_id = parent.map(str::to_owned);
        let mut session = ActiveSession::new(meta.clone(), false, Vec::new());
        let (commands, receiver) = smol::channel::unbounded();
        session.runtime = Runtime::Live(commands);
        session.live_approval_mode = Some(meta.approval_mode);
        state.sessions.push(meta);
        state.residents.parked.insert(id.into(), session);
        receiver
    }

    fn completed(id: &str, status: TurnStatus) -> AgentEvent {
        AgentEvent::TurnCompleted {
            turn_id: id.into(),
            status,
            usage: None,
        }
    }

    fn live(state: &mut AppState, id: &str, event: AgentEvent, cx: &mut HostCx) {
        state.on_event(id, event, cx);
        state.reconcile_attention(cx);
    }

    fn alerts(cx: &mut TestAppContext) -> Vec<(String, ThreadAttentionKind)> {
        cx.drain_outgoing()
            .into_iter()
            .filter_map(|message| match message {
                HostMessage::Event(EventEnvelope {
                    topic: Topic::RuntimeEvents,
                    event:
                        ServerEvent::Runtime(RuntimeEvent::ThreadAttention {
                            session_id,
                            title,
                            kind,
                        }),
                    ..
                }) => {
                    assert_eq!(title, format!("Title {session_id}"));
                    Some((session_id, kind))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn completion_waits_for_queue_background_work_and_child_callback() {
        let store = TestStore::new("attention-settled");
        let mut cx = TestAppContext::default();
        let state = cx.new_entity(TestClientState::new((*store).clone()));
        let (parent_commands, _child_commands) = state.update(&mut cx, |state, cx| {
            let parent = install(state, "parent", None);
            let child = install(state, "child", Some("parent"));
            live(
                state,
                "parent",
                AgentEvent::TurnStarted {
                    turn_id: "first".into(),
                },
                cx,
            );
            state
                .resident_mut("parent")
                .unwrap()
                .push_queued("next".into(), Vec::new());
            live(
                state,
                "parent",
                AgentEvent::BackgroundTasksChanged { count: 1 },
                cx,
            );
            live(
                state,
                "child",
                AgentEvent::TurnStarted {
                    turn_id: "child-turn".into(),
                },
                cx,
            );
            live(
                state,
                "parent",
                completed("first", TurnStatus::Completed),
                cx,
            );
            (parent, child)
        });
        assert!(alerts(&mut cx).is_empty());
        let SessionCommand::SendTurn { delivery_id, .. } = parent_commands.try_recv().unwrap()
        else {
            panic!("queue must dispatch")
        };
        state.update(&mut cx, |state, cx| {
            live(
                state,
                "parent",
                AgentEvent::TurnAccepted { delivery_id },
                cx,
            );
            live(
                state,
                "parent",
                AgentEvent::TurnStarted {
                    turn_id: "second".into(),
                },
                cx,
            );
            live(
                state,
                "parent",
                completed("second", TurnStatus::Completed),
                cx,
            );
            live(
                state,
                "parent",
                AgentEvent::BackgroundTasksChanged { count: 0 },
                cx,
            );
            live(
                state,
                "child",
                completed("child-turn", TurnStatus::Completed),
                cx,
            );
            assert!(state.pending_child_callbacks.contains_key("child"));
        });
        assert!(
            alerts(&mut cx).is_empty(),
            "the callback can enqueue parent work"
        );
        cx.run_until_parked();
        assert!(alerts(&mut cx).is_empty());
        let SessionCommand::SendTurn { delivery_id, .. } = parent_commands.try_recv().unwrap()
        else {
            panic!("child callback must dispatch")
        };
        state.update(&mut cx, |state, cx| {
            live(
                state,
                "parent",
                AgentEvent::TurnAccepted { delivery_id },
                cx,
            );
            live(
                state,
                "parent",
                AgentEvent::TurnStarted {
                    turn_id: "callback".into(),
                },
                cx,
            );
            live(
                state,
                "parent",
                completed("callback", TurnStatus::Completed),
                cx,
            );
            live(
                state,
                "parent",
                completed("callback", TurnStatus::Completed),
                cx,
            );
        });
        assert_eq!(
            alerts(&mut cx),
            [("parent".into(), ThreadAttentionKind::Completed)]
        );
    }

    #[test]
    fn failures_interrupts_shutdown_and_replay_never_complete() {
        let store = TestStore::new("attention-outcomes");
        let mut state = AppState::with_ai_titles((*store).clone(), false);
        let mut context = TestAppContext::default();
        let mut cx = context.host_cx();
        let _commands = install(&mut state, "main", None);
        for (index, status) in [
            TurnStatus::Failed,
            TurnStatus::Interrupted,
            TurnStatus::Completed,
        ]
        .into_iter()
        .enumerate()
        {
            let turn = index.to_string();
            live(
                &mut state,
                "main",
                AgentEvent::TurnStarted {
                    turn_id: turn.clone(),
                },
                &mut cx,
            );
            if status == TurnStatus::Completed {
                state.interrupt("main", &mut cx).unwrap();
            }
            live(&mut state, "main", completed(&turn, status), &mut cx);
        }
        live(
            &mut state,
            "main",
            AgentEvent::TurnStarted {
                turn_id: "shutdown".into(),
            },
            &mut cx,
        );
        live(
            &mut state,
            "main",
            AgentEvent::BackgroundTasksChanged { count: 1 },
            &mut cx,
        );
        live(
            &mut state,
            "main",
            completed("shutdown", TurnStatus::Completed),
            &mut cx,
        );
        live(
            &mut state,
            "main",
            AgentEvent::SessionClosed { reason: None },
            &mut cx,
        );
        let _commands = install(&mut state, "history", None);
        state.resident_mut("history").unwrap().timeline = Timeline::fold_events([
            AgentEvent::TurnStarted {
                turn_id: "old".into(),
            },
            completed("old", TurnStatus::Completed),
            AgentEvent::UserInputRequested {
                request_id: "old-question".into(),
                questions: Vec::new(),
                delivery: agent::UserInputDelivery::Blocking,
            },
        ]);
        state.reconcile_attention(&mut cx);
        assert!(alerts(&mut context).is_empty());
    }

    #[test]
    fn structured_main_and_child_requests_notify_only_when_new() {
        let store = TestStore::new("attention-requests");
        let mut state = AppState::with_ai_titles((*store).clone(), false);
        let mut context = TestAppContext::default();
        let mut cx = context.host_cx();
        let _main = install(&mut state, "main", None);
        let _child = install(&mut state, "child", Some("main"));
        for id in ["main", "child"] {
            let question = AgentEvent::UserInputRequested {
                request_id: "q".into(),
                questions: Vec::new(),
                delivery: agent::UserInputDelivery::Blocking,
            };
            let approval = AgentEvent::ApprovalRequested(agent::ApprovalRequest {
                id: "a".into(),
                turn_id: None,
                kind: agent::ApprovalKind::ExecCommand {
                    command: "private details".into(),
                    cwd: None,
                    reason: None,
                },
                options: Vec::new(),
            });
            for event in [question.clone(), question, approval.clone(), approval] {
                live(&mut state, id, event, &mut cx);
            }
        }
        assert_eq!(
            alerts(&mut context),
            [
                ("main".into(), ThreadAttentionKind::Question),
                ("main".into(), ThreadAttentionKind::Approval),
                ("child".into(), ThreadAttentionKind::Question),
                ("child".into(), ThreadAttentionKind::Approval),
            ]
        );
    }
}
