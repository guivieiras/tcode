use super::*;
use gpui::SystemNotification;
use tcode_protocol::ThreadAttentionKind;

#[derive(Default)]
pub(super) struct ThreadNotifications {
    generation: u64,
    alerts: HashMap<String, (String, ThreadAttentionKind)>,
}

impl ThreadNotifications {
    pub(super) fn close(&mut self, cx: &App) {
        for tag in self.alerts.keys() {
            cx.dismiss_system_notification(tag);
        }
        self.alerts.clear();
        self.generation += 1;
    }
}

impl AppShell {
    pub(super) fn register_thread_notifications(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.on_release(|shell, cx| shell.thread_notifications.close(cx))
            .detach();
        let shell = cx.weak_entity();
        let handle = window.window_handle();
        cx.on_system_notification_response(move |response, cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.activate_thread_notification(response.tag.as_ref(), window, cx);
                });
            });
        });
        self._subscriptions
            .push(cx.observe_window_activation(window, |shell, window, cx| {
                shell.reconcile_thread_notifications(window, cx);
            }));
    }

    fn thread_visible(&self, id: &str, window: &Window, cx: &App) -> bool {
        let state = self.window_state.read(cx);
        window.is_window_active()
            && self
                .store()
                .is_some_and(|store| store.read(cx).active_session_id().as_deref() == Some(id))
            && if self.compact(cx) {
                state.destination() == Destination::Thread
            } else {
                state.route() == Route::Chat
            }
    }

    pub(super) fn show_thread_attention(
        &mut self,
        id: &str,
        title: &str,
        kind: ThreadAttentionKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.store() else { return };
        if !store.read(cx).thread_notifications_enabled()
            || !store.read(cx).contains_session(id)
            || self.thread_visible(id, window, cx)
        {
            return;
        }
        let heading = match kind {
            ThreadAttentionKind::Completed => crate::tr!("desktop_notifications.completed"),
            ThreadAttentionKind::Question => crate::tr!("desktop_notifications.question"),
            ThreadAttentionKind::Approval => crate::tr!("desktop_notifications.approval"),
        };
        let tag = format!("thread:{}:{id}", self.thread_notifications.generation);
        self.thread_notifications
            .alerts
            .insert(tag.clone(), (id.to_owned(), kind));
        cx.show_system_notification(SystemNotification {
            tag: tag.into(),
            title: heading.into_owned().into(),
            body: title.to_owned().into(),
            actions: Vec::new(),
        });
    }

    pub(super) fn reconcile_thread_notifications(&mut self, window: &Window, cx: &App) {
        let remove: Vec<_> = self
            .thread_notifications
            .alerts
            .iter()
            .filter(|(_, (id, kind))| {
                self.store().is_none_or(|store| {
                    let store = store.read(cx);
                    !store.thread_notifications_enabled()
                        || !store.contains_session(id)
                        || self.thread_visible(id, window, cx)
                        || match kind {
                            ThreadAttentionKind::Completed => false,
                            ThreadAttentionKind::Question => !store.pending_user_input_for(id),
                            ThreadAttentionKind::Approval => !store.pending_approval_for(id),
                        }
                })
            })
            .map(|(tag, _)| tag.clone())
            .collect();
        for tag in remove {
            cx.dismiss_system_notification(&tag);
            self.thread_notifications.alerts.remove(&tag);
        }
    }

    fn activate_thread_notification(
        &mut self,
        tag: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, _)) = self.thread_notifications.alerts.get(tag).cloned() else {
            return;
        };
        let Some(store) = self.store() else { return };
        if !store.read(cx).contains_session(&id) || !store.read(cx).thread_notifications_enabled() {
            return;
        }
        cx.activate(true);
        window.activate_window();
        store.update(cx, |store, _| store.select_session(id));
        self.go(Destination::Thread, cx);
        self.open_thread(window, cx);
        self.reconcile_thread_notifications(window, cx);
    }
}
