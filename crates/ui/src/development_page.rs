//! Renders host replicas; build jobs and APK transfers outlive this view.
use crate::{
    overlay::{DialogButtons, OverlayExt as _},
    scroll::ScrollableElement as _,
    store::WorkspaceStore,
    theme::ActiveTheme as _,
    widgets::{
        button::{Button, ButtonVariant, ButtonVariants as _},
        input::{Input, InputEvent, InputState},
    },
};
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _,
};
use gpui_base::{h_flex, v_flex};
use tcode_client::host::{ApkDownloadState, ApkInstallerState};
use tcode_protocol::{
    BuildAttemptState, Command, DevelopmentRestartState, DevelopmentTarget, SettingsPatch,
};

pub struct DevelopmentPage {
    store: Entity<WorkspaceStore>,
    checkout: Entity<InputState>,
    pushed_checkout: String,
    dirty: bool,
    expanded: [bool; 2],
    confirm_restart: bool,
    request_pending: bool,
    error: Option<String>,
    installer_message: Option<String>,
    _subscriptions: Vec<Subscription>,
    _tick: gpui::Task<()>,
}
impl DevelopmentPage {
    pub fn new(store: Entity<WorkspaceStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let checkout = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::tr!("development.checkout_hint"))
        });
        let subscription = cx.subscribe(&checkout, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change)
                && this.checkout.read(cx).value().as_str() != this.pushed_checkout
            {
                this.dirty = true;
            }
        });
        let observe = cx.observe(&store, |_, _, cx| cx.notify());
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            store,
            checkout,
            pushed_checkout: String::new(),
            dirty: false,
            expanded: [false; 2],
            confirm_restart: false,
            request_pending: false,
            error: None,
            installer_message: None,
            _subscriptions: vec![subscription, observe],
            _tick: tick,
        }
    }

    fn request(&mut self, command: Command, cx: &mut Context<Self>) {
        let checkout_change = matches!(command, Command::PatchSettings { .. });
        self.request_pending = true;
        self.error = None;
        let task = self
            .store
            .update(cx, |store, cx| store.development_command(command, cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.request_pending = false;
                if result.is_ok() && checkout_change {
                    this.dirty = false;
                }
                if let Err(error) = result {
                    if error.code == "development_confirmation_required" {
                        this.confirm_restart = true;
                    } else {
                        this.error = Some(error.message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn restart(&mut self, allow_interrupt: bool, cx: &mut Context<Self>) {
        let state = self.store.read(cx).development.clone();
        let Some(artifact) = &state.targets[0].artifact else {
            return;
        };
        self.confirm_restart = false;
        self.request(
            Command::RestartDevelopmentDesktop {
                host_instance_id: state.host_instance_id,
                build_id: artifact.build_id.clone(),
                allow_interrupt,
            },
            cx,
        );
    }
}

impl Render for DevelopmentPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let state = store.development.clone();
        let connected = store.settings_connection_state().is_connected();
        let native_apk = store.supports_apk_install();
        let transfer = state.targets[1]
            .artifact
            .as_ref()
            .map(|artifact| store.apk_download_state(&artifact.build_id))
            .unwrap_or_default();
        let restart_busy = matches!(
            state.restart,
            DevelopmentRestartState::Preparing | DevelopmentRestartState::Restarting
        );
        let disabled = !connected
            || !state.available
            || state.building()
            || restart_busy
            || self.request_pending;
        let checkout_value = state
            .checkout
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !self.dirty && self.pushed_checkout != checkout_value {
            self.pushed_checkout = checkout_value.clone();
            self.checkout
                .update(cx, |input, cx| input.set_value(checkout_value, window, cx));
        }
        let mut page = v_flex()
            .w_full()
            .gap_5()
            .child(div().text_xl().child(crate::tr!("development.title")))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(state.host_name.clone()),
            )
            .when(!state.available, |page| {
                page.child(
                    state
                        .unavailable_reason
                        .clone()
                        .unwrap_or_else(|| crate::tr!("development.unavailable").to_string()),
                )
            })
            .child(
                v_flex()
                    .gap_2()
                    .child(crate::tr!("development.checkout"))
                    .child(Input::new(&self.checkout).disabled(disabled))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(crate::tr!("development.source_note")),
                    )
                    .child(
                        Button::new("save-development-checkout")
                            .label(crate::tr!("development.save"))
                            .disabled(disabled || !self.dirty)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let value = this.checkout.read(cx).value().trim().to_owned();
                                this.request(
                                    Command::PatchSettings {
                                        patch: SettingsPatch::DevelopmentCheckout(
                                            if value.is_empty() {
                                                None
                                            } else {
                                                Some(value.into())
                                            },
                                        ),
                                    },
                                    cx,
                                );
                            })),
                    ),
            );
        for target in [DevelopmentTarget::Desktop, DevelopmentTarget::Android] {
            let index = target.index();
            let target_state = &state.targets[index];
            let title = if index == 0 {
                crate::tr!("development.desktop")
            } else {
                crate::tr!("development.android")
            };
            let build_label = if index == 0 {
                crate::tr!("development.rebuild")
            } else {
                crate::tr!("development.build_apk")
            };
            let instance = state.host_instance_id.clone();
            let mut actions = h_flex().flex_wrap().gap_2().child(
                Button::new(("development-build", index))
                    .label(build_label)
                    .primary()
                    .disabled(disabled || state.checkout.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.request(
                            Command::StartDevelopmentBuild {
                                host_instance_id: instance.clone(),
                                target,
                            },
                            cx,
                        );
                    })),
            );
            let mut section = v_flex()
                .w_full()
                .gap_3()
                .p_4()
                .border_1()
                .border_color(cx.theme().border)
                .rounded_lg()
                .child(div().text_lg().child(title));
            if index == 0 {
                let running = target_state.artifact.as_ref().is_some_and(|artifact| {
                    Some(&artifact.build_id) == state.running_build_id.as_ref()
                });
                let label = if running {
                    crate::tr!("development.running")
                } else if target_state.artifact.is_some() {
                    crate::tr!("development.ready_restart")
                } else {
                    crate::tr!("development.running_unknown")
                };
                section = section.child(label);
                if let Some(id) = &state.running_build_id {
                    section = section.child(div().text_xs().child(crate::tr!(
                        "development.running_id",
                        id = &id[..12.min(id.len())]
                    )));
                }
                actions = actions.child(
                    Button::new("development-restart")
                        .label(crate::tr!("development.restart"))
                        .disabled(disabled || target_state.artifact.is_none() || running)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.store.read(cx).development.active_work {
                                this.confirm_restart = true;
                                cx.notify();
                            } else {
                                this.restart(false, cx);
                            }
                        })),
                );
            }
            if let Some(artifact) = &target_state.artifact {
                section = section.child(div().text_sm().child(crate::tr!(
                    "development.artifact",
                    id = &artifact.build_id[..12.min(artifact.build_id.len())],
                    size = format!("{:.1}", artifact.size as f64 / 1048576.0)
                )));
                if index == 1 {
                    if native_apk {
                        let artifact = artifact.clone();
                        let ready = transfer == ApkDownloadState::Ready;
                        let downloading = matches!(transfer, ApkDownloadState::Downloading { .. });
                        let failed = matches!(transfer, ApkDownloadState::Failed(_));
                        actions = actions.child(
                            Button::new("development-download")
                                .label(if failed {
                                    crate::tr!("development.retry")
                                } else {
                                    crate::tr!("development.download")
                                })
                                .disabled(!connected || downloading || ready)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let task = this.store.update(cx, |store, cx| {
                                        store.download_apk(artifact.clone(), cx)
                                    });
                                    cx.spawn(async move |this, cx| {
                                        let result = task.await;
                                        let _ = this.update(cx, |this, cx| {
                                            this.error = result.err();
                                            cx.notify();
                                        });
                                    })
                                    .detach();
                                })),
                        );
                        let build = target_state.artifact.as_ref().unwrap().build_id.clone();
                        actions = actions.child(
                            Button::new("development-install")
                                .label(crate::tr!("development.install"))
                                .disabled(!ready || self.request_pending)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.request_pending = true;
                                    this.installer_message = Some(
                                        crate::tr!("development.installer_opening").to_string(),
                                    );
                                    let task = this.store.update(cx, |store, cx| {
                                        store.open_apk_installer(build.clone(), cx)
                                    });
                                    cx.spawn(async move |this, cx| {
                                        let result = task.await;
                                        let _ = this.update(cx, |this, cx| {
                                            this.request_pending = false;
                                            match result {
                                                Ok(ApkInstallerState::ConfirmationOpened) => {
                                                    this.installer_message = Some(
                                                        crate::tr!(
                                                            "development.installer_confirmation"
                                                        )
                                                        .to_string(),
                                                    )
                                                }
                                                Ok(ApkInstallerState::Installed) => {
                                                    this.installer_message = Some(
                                                        crate::tr!("development.installed")
                                                            .to_string(),
                                                    )
                                                }
                                                Err(error) => {
                                                    this.installer_message = None;
                                                    this.error = Some(error);
                                                }
                                            }
                                            cx.notify();
                                        });
                                    })
                                    .detach();
                                })),
                        );
                    } else {
                        let path = artifact.path.to_string_lossy().into_owned();
                        section = section.child(div().text_sm().child(path.clone()));
                        actions = actions.child(
                            Button::new("development-copy-apk")
                                .label(crate::tr!("development.copy_path"))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        path.clone(),
                                    ))
                                }),
                        );
                    }
                }
            }
            section = section.child(actions);
            if let Some(attempt) = &target_state.attempt {
                let label = match attempt.state {
                    BuildAttemptState::Running => crate::tr!("development.building"),
                    BuildAttemptState::Succeeded => crate::tr!("development.succeeded"),
                    BuildAttemptState::Failed => crate::tr!("development.failed"),
                    BuildAttemptState::Interrupted => crate::tr!("development.interrupted"),
                };
                section = section.child(div().text_sm().child(label));
                if let Some(completed) = attempt.completed_at {
                    section = section.child(div().text_sm().child(crate::tr!(
                        "development.completed",
                        time = crate::time::humanize_ago(
                            crate::time::now_secs().saturating_sub(completed)
                        )
                    )));
                } else if connected {
                    section = section.child(div().text_sm().child(crate::tr!(
                        "development.elapsed",
                        seconds = crate::time::now_secs().saturating_sub(attempt.started_at)
                    )));
                }
                if let Some(error) = &attempt.error {
                    section = section.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(error.clone()),
                    );
                }
                section = section.child(
                    Button::new(("development-output", index))
                        .label(if self.expanded[index] {
                            crate::tr!("development.hide_output")
                        } else {
                            crate::tr!("development.show_output")
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.expanded[index] = !this.expanded[index];
                            cx.notify();
                        })),
                );
                if self.expanded[index] {
                    if attempt.output_truncated {
                        section = section
                            .child(div().text_sm().child(crate::tr!("development.truncated")));
                    }
                    section = section.child(
                        div()
                            .id(("development-log", index))
                            .max_h(gpui::px(300.))
                            .overflow_y_scroll_area()
                            .text_xs()
                            .child(attempt.output.clone()),
                    );
                }
            }
            page = page.child(section);
        }
        if self.confirm_restart {
            self.confirm_restart = false;
            cx.defer_in(window, |_, window, cx| {
                let page = cx.entity();
                window.open_alert_dialog(cx, move |alert, _, cx| {
                    let page = page.clone();
                    alert
                        .bg(cx.theme().popover)
                        .title(crate::tr!("development.restart"))
                        .description(crate::tr!("development.confirm_warning"))
                        .button_props(
                            DialogButtons::default()
                                .ok_variant(ButtonVariant::Danger)
                                .ok_text(crate::tr!("development.confirm_restart"))
                                .cancel_text(crate::tr!("development.cancel"))
                                .show_cancel(true),
                        )
                        .on_ok(move |_, _, cx| {
                            page.update(cx, |page, cx| page.restart(true, cx));
                            true
                        })
                });
            });
        }
        match &state.restart {
            DevelopmentRestartState::Preparing => {
                page = page.child(crate::tr!("development.preparing"))
            }
            DevelopmentRestartState::Restarting if connected => {
                page = page.child(crate::tr!("development.restarting"))
            }
            DevelopmentRestartState::Failed(error) => page = page.child(error.clone()),
            _ => {}
        }
        match transfer {
            ApkDownloadState::Downloading { received, total } => {
                page = page.child(crate::tr!(
                    "development.transfer",
                    received = received,
                    total = total
                ))
            }
            ApkDownloadState::Failed(error) => page = page.child(error),
            _ => {}
        }
        if let Some(message) = &self.installer_message {
            page = page.child(message.clone());
        }
        if let Some(error) = &self.error {
            page = page.child(div().text_color(cx.theme().danger).child(error.clone()));
        }
        page
    }
}
