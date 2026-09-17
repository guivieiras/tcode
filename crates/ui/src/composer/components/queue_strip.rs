use super::super::*;
use crate::sizing::design;
use crate::touch_scroll::TouchScrollExt as _;
#[cfg(not(target_family = "wasm"))]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(target_family = "wasm")]
use web_time::{SystemTime, UNIX_EPOCH};

/// Queue previews collapse whitespace and clip long messages (the full text is
/// still what gets sent).
fn truncate_queued(text: &str) -> String {
    const MAX: usize = 80;
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX {
        return normalized;
    }
    let clipped: String = normalized.chars().take(MAX).collect();
    format!("{clipped}…")
}

impl Composer {
    fn edit_queued(
        &mut self,
        message: tcode_protocol::QueuedMessageStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Whitespace and image jobs are drafts too, even before they are sendable.
        let has_draft = !self.input.read(cx).value().is_empty()
            || self.pending_image_loads > 0
            || self.has_sendable_content(cx);
        if !has_draft {
            self.take_queued_and_refill(message, window, cx);
            return;
        }
        let composer = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let composer = composer.clone();
            let message = message.clone();
            alert
                .bg(cx.theme().popover)
                .title(crate::tr!("composer.replace_draft_title"))
                .description(crate::tr!("composer.replace_draft_description"))
                .button_props(
                    DialogButtons::default()
                        .ok_text(crate::tr!("composer.replace_draft"))
                        .cancel_text(crate::tr!("composer.relay_cancel"))
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    window.close_dialog(cx);
                    composer.update(cx, |composer, cx| {
                        composer.take_queued_and_refill(message.clone(), window, cx);
                    });
                    // The confirmation was replaced by the pending-removal dialog.
                    false
                })
        });
    }

    fn take_queued_and_refill(
        &mut self,
        message: tcode_protocol::QueuedMessageStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Keep the confirmed draft stable while a remote host acknowledges removal.
        // The modal prevents typing or navigation from racing the replacement.
        window.open_alert_dialog(cx, |alert, _, cx| {
            alert
                .bg(cx.theme().popover)
                .title(crate::tr!("composer.edit_queued"))
                .description(crate::tr!("composer.restoring_queued"))
                .footer(Spinner::new().small())
                .keyboard(false)
                .on_ok(|_, _, _| false)
                .on_cancel(|_, _, _| false)
        });
        let removed = self
            .workspace_store
            .update(cx, |store, cx| store.take_queued(message.id, cx));
        cx.spawn_in(window, async move |this, cx| {
            let result = removed.await;
            let _ = this.update_in(cx, |this, window, cx| {
                window.close_dialog(cx);
                if let Err(error) = result {
                    window.push_notification(Notification::error(error.message), cx);
                    return;
                }
                let state = this.workspace_store.read(cx).composer_state();
                let review_count = this.workspace_store.read(cx).review_comments().len();
                this.workspace_store.update(cx, |store, _| {
                    for context in state.terminal_contexts {
                        store.remove_terminal_context(context.id);
                    }
                    for index in (0..review_count).rev() {
                        store.remove_review_comment(index);
                    }
                });
                this.sync_images_session(cx);
                this.image_load_generation = this.image_load_generation.wrapping_add(1);
                this.pending_image_loads = 0;
                this.pending_images = message
                    .attachment_paths
                    .into_iter()
                    .map(|path| {
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        PendingImage { path, name }
                    })
                    .collect();
                #[cfg(all(feature = "voice", target_os = "macos"))]
                this.abort_dictation(cx);
                this.set_input_text(message.text, window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Queued messages with edit, send-now and removal actions. Scheduled
    /// messages also show a live countdown.
    pub(in super::super) fn render_queue_strip(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Some(queue) = self.workspace_store.read(cx).composer_state().queue else {
            self.scheduled_countdown_tick = None;
            return None;
        };
        let deliveries = self.workspace_store.read(cx).delivery_messages();
        let queued: Vec<_> = queue
            .messages
            .into_iter()
            .filter(|message| {
                !deliveries.iter().any(|(key, text, failure, acknowledged)| {
                    !acknowledged
                        && failure.is_none()
                        && (message.delivery_key.as_deref() == Some(key.as_str())
                            || (message.delivery_key.is_none() && message.text == *text))
                })
            })
            .collect();
        let can_steer = queue.can_steer;
        let agent = queue.agent;
        let has_scheduled = queued
            .iter()
            .any(|message| message.fire_at_unix_secs.is_some());
        if has_scheduled && self.scheduled_countdown_tick.is_none() {
            self.scheduled_countdown_tick = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        } else if !has_scheduled {
            // Dropping the task cancels its timer, so at most one repaint loop
            // exists and it stops as soon as the last scheduled row disappears.
            self.scheduled_countdown_tick = None;
        }
        if queued.is_empty() {
            return None;
        }

        let muted = cx.theme().muted_foreground;
        let mut strip = v_flex()
            .w_full()
            .gap_1()
            .when(self.compact, |el| el.gap_0())
            .child(
                div()
                    .flex_none()
                    .px_1()
                    .text_size(design(11.))
                    .text_color(muted)
                    .child(crate::tr!("composer.queued_count", count = queued.len())),
            );

        for (index, message) in queued.into_iter().enumerate() {
            let id = message.id;
            let scheduled = message.fire_at_unix_secs.is_some();
            let steer_tooltip = if scheduled {
                crate::tr!("composer.send_now").into_owned()
            } else if can_steer {
                crate::tr!("composer.steer_queued").into_owned()
            } else {
                crate::tr!("composer.steer_unsupported_tooltip", agent = agent).into_owned()
            };
            let countdown = message.fire_at_unix_secs.map(|fire_at| {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                format_countdown(fire_at.saturating_sub(now))
            });
            strip = strip.child(
                h_flex()
                    .flex_none()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .px_1()
                    .when(index > 0, |row| {
                        row.when(!self.compact, |row| row.pt_1())
                            .border_t_1()
                            .border_color(cx.theme().border)
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(design(if self.compact { 12. } else { 13. }))
                            .text_color(cx.theme().foreground)
                            .child(truncate_queued(&message.text)),
                    )
                    .when_some(countdown, |row, countdown| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(design(12.))
                                .text_color(muted)
                                .child(countdown),
                        )
                    })
                    .child(
                        Button::new(("queue-steer", id as usize))
                            .ghost()
                            .xsmall()
                            .when(self.compact, |button| button.min_h(design(32.)))
                            .icon(IconName::ArrowUp)
                            // Scheduled rows always support send-now: the
                            // runtime removes the deadline and uses the normal
                            // send/queue path even when native steering is absent.
                            .disabled(!self.interactive(cx) || (!scheduled && !can_steer))
                            .tooltip(steer_tooltip)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.workspace_store
                                    .update(cx, |store, _cx| store.steer_queued(id));
                            })),
                    )
                    .child(
                        Button::new(("queue-edit", id as usize))
                            .ghost()
                            .xsmall()
                            .when(self.compact, |button| button.min_h(design(32.)))
                            .icon(Icon::empty().path("icons/pencil.svg"))
                            .disabled(!self.interactive(cx))
                            .tooltip(crate::tr!("composer.edit_queued"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_queued(message.clone(), window, cx);
                            })),
                    )
                    .child(
                        Button::new(("queue-drop", id as usize))
                            .ghost()
                            .xsmall()
                            .when(self.compact, |button| button.min_h(design(32.)))
                            .icon(IconName::Close)
                            .disabled(!self.interactive(cx))
                            .tooltip(crate::tr!("composer.drop_queued"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.workspace_store
                                    .update(cx, |store, _| store.drop_queued(id));
                            })),
                    ),
            );
        }
        Some(
            div()
                .debug_selector(|| "composer-queue-drawer".into())
                .mx_2()
                .px_2()
                .py_1p5()
                .when(self.compact, |el| el.py_1())
                .min_w_0()
                .rounded_t(design(12.))
                .border_1()
                .border_b_0()
                .border_color(cx.theme().border)
                .bg(cx.theme().muted)
                .child(
                    div()
                        .id("queued-messages-scroll")
                        .w_full()
                        .max_h(design(180.))
                        .touch_overflow_y_scroll()
                        .child(strip),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::OverlayHost;
    use gpui::{TestAppContext, VisualTestContext};
    use tcode_core::project::SessionMeta;
    use tcode_runtime::pipe::{HostServices, spawn_host};
    use tcode_services::store::SessionStore;

    struct DialogBody;
    impl Render for DialogBody {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn draw(cx: &mut VisualTestContext, store: &Entity<WorkspaceStore>) {
        store.update(cx, |store, cx| store.drain_host_events_for_test(cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// Taking a message back must protect the existing draft, restore images,
    /// and leave the draft untouched if delivery won the race with Edit.
    #[gpui::test]
    fn queued_edit_confirms_replacement_and_restores_only_after_removal(cx: &mut TestAppContext) {
        cx.update(crate::theme::init);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp")
            .join(format!(
                "queued-edit-test-{}",
                tcode_services::store::now_millis()
            ));
        let disk = SessionStore::open_at(root.clone()).unwrap();
        let meta = SessionMeta::new(ProviderKind::Codex, root.clone(), None);
        disk.upsert_meta(&meta).unwrap();
        let host = spawn_host(disk, HostServices::default()).unwrap();
        let store = cx.new(|cx| WorkspaceStore::new(host.link(), cx));
        store.update(cx, |store, _| store.select_session(meta.id.clone()));
        let mut composer = None;
        let (_overlay, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| Composer::new(store.clone(), window, cx));
            composer = Some(view);
            // Exercise the composer actions without starting asynchronous thumbnail loads.
            let body = cx.new(|_| DialogBody);
            OverlayHost::new(body, window, cx)
        });
        let composer = composer.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !store.read_with(cx, |store, _| store.composer_state().has_active_session) {
            assert!(std::time::Instant::now() < deadline, "session did not load");
            draw(cx, &store);
            std::thread::sleep(Duration::from_millis(1));
        }
        let image_path = root.join("queued.png");
        image::RgbaImage::new(1, 1).save(&image_path).unwrap();
        let target = meta.id.clone();
        let attachment = image_path.clone();
        let message = smol::block_on(host.update_state_for_test(move |state, cx| {
            state.queue_message_for_replica_test(
                &target,
                "queued text".into(),
                vec![attachment],
                cx,
            );
            state
                .session_status_snapshot(&target)
                .unwrap()
                .queued_messages[0]
                .clone()
        }))
        .unwrap();
        assert_eq!(message.attachment_paths, vec![image_path.clone()]);
        draw(cx, &store);
        cx.update(|window, cx| {
            composer.update(cx, |composer, cx| {
                composer.set_draft("existing draft", window, cx);
                composer.edit_queued(message.clone(), window, cx);
            })
        });
        draw(cx, &store);
        cx.simulate_keystrokes("escape");
        draw(cx, &store);
        assert_eq!(
            composer.read_with(cx, |composer, cx| composer.draft(cx)),
            "existing draft"
        );
        let target = meta.id.clone();
        assert_eq!(
            smol::block_on(host.update_state_for_test(move |state, _| {
                state
                    .session_status_snapshot(&target)
                    .unwrap()
                    .queued_messages
                    .len()
            }))
            .unwrap(),
            1,
            "Cancel must not remove the queued message"
        );

        cx.update(|window, cx| {
            composer.update(cx, |composer, cx| {
                composer.edit_queued(message.clone(), window, cx);
            })
        });
        draw(cx, &store);
        cx.simulate_keystrokes("enter");
        draw(cx, &store);
        assert_eq!(
            composer.read_with(cx, |composer, cx| composer.draft(cx)),
            "queued text"
        );
        composer.read_with(cx, |composer, _| {
            assert_eq!(composer.pending_images.len(), 1);
            assert_eq!(composer.pending_images[0].path, image_path);
        });

        // A stale row must not overwrite a newer draft, even after confirmation.
        cx.update(|window, cx| {
            composer.update(cx, |composer, cx| {
                composer.set_draft("newer draft", window, cx);
                composer.edit_queued(message, window, cx);
            })
        });
        draw(cx, &store);
        cx.simulate_keystrokes("enter");
        draw(cx, &store);
        assert_eq!(
            composer.read_with(cx, |composer, cx| composer.draft(cx)),
            "newer draft"
        );
        composer.read_with(cx, |composer, _| {
            assert_eq!(composer.pending_images.len(), 1)
        });
        host.shutdown_blocking().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
