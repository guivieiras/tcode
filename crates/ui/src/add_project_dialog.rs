//! Add Project: pick a directory on the *host* and, optionally, import the
//! external-agent threads it already has.
//!
//! Everything here is host state reached over the pipe — the recents scan, the
//! project root and the import run. The only native step is the directory
//! picker, which browses *this* machine; it is offered when this build has one
//! and the attachment is local, and never as a substitute for the host's own
//! judgment about a path.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::overlay::{DialogActions, OverlayExt as _};
use crate::scroll::ScrollableElement as _;
use crate::sizing::fit_viewport;
use crate::theme::ActiveTheme as _;
use crate::widgets::button::{Button, ButtonVariants as _};
use crate::widgets::input::{Input, InputState};
use crate::widgets::progress::Progress;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_base::{StyledExt as _, h_flex, v_flex};

use crate::store::{TopicKind, WorkspaceStore, observe_store_topics};
use crate::time::{humanize_ago, now_secs};
use tcode_protocol::{CommandResponse, ExternalImportState, ExternalThread, RecentDir, SourceTool};

const RECENT_LIMIT: usize = 15;
const RECENT_ROW_HEIGHT_ESTIMATE: f32 = 64.;
const RECENT_VIEWPORT_MAX_HEIGHT: f32 = 390.;
/// Everything above the recents viewport inside the dialog: title, the path row
/// and the footer. Subtracted so the list scrolls instead of pushing the Open
/// button off a short window.
const RECENT_VIEWPORT_CHROME: f32 = 260.;

enum RecentState {
    Loading,
    Ready(Vec<RecentDir>),
    Failed(String),
}

#[derive(Clone)]
enum ImportChoice {
    Checking,
    T3 {
        history: tcode_protocol::T3ProjectHistory,
        profiles: BTreeMap<String, String>,
    },
    Native,
    Failed(String),
}

#[derive(Clone)]
struct RecentConfirmation {
    recent: RecentDir,
    choice: ImportChoice,
}

/// Whether this build can put up a directory picker at all.
const NATIVE_DIRECTORY_PICKER: bool = cfg!(feature = "native-dialogs");

pub(super) struct AddProjectDialog {
    store: Entity<WorkspaceStore>,
    path_input: Entity<InputState>,
    recent: RecentState,
    scan_warning: Option<String>,
    /// The last failure to show under the path row. Host-authored where the
    /// host produced it, so the user reads the host's own reason.
    error: Option<String>,
    confirmation: Option<RecentConfirmation>,
    busy: bool,
}

pub(super) fn open(store: Entity<WorkspaceStore>, window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|cx| AddProjectDialog::new(store, window, cx));
    dialog.update(cx, |dialog, cx| dialog.scan(cx));
    let content = dialog.clone();
    window.open_dialog(cx, move |builder, _, cx| {
        let dialog_content = content.clone();
        builder
            .w(px(680.))
            .rounded(crate::material::radius_overlay())
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_xl()
            .title(crate::tr!("sidebar.add_project").into_owned())
            .content(move |content_el, _, _| content_el.child(dialog_content.clone()))
    });
}

impl AddProjectDialog {
    fn new(store: Entity<WorkspaceStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = match store.read(cx).remote_host_name() {
            Some(host) => crate::tr!("sidebar.host_path_placeholder", host = host).into_owned(),
            None => crate::tr!("sidebar.path_placeholder").into_owned(),
        };
        let path_input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        Self {
            store,
            path_input,
            recent: RecentState::Loading,
            scan_warning: None,
            error: None,
            confirmation: None,
            busy: false,
        }
    }

    /// Whether the platform directory picker is meaningful here. It browses this
    /// machine, so a remote attachment must type a host path instead — the
    /// picker would silently produce a path the host cannot resolve.
    fn can_browse(&self, cx: &App) -> bool {
        NATIVE_DIRECTORY_PICKER && !self.store.read(cx).is_remote()
    }

    fn scan(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let recent = store.update(cx, |store, cx| store.scan_external_history(cx));
        cx.spawn(async move |this, cx| {
            let recent = recent.await;
            let _ = this.update(cx, |dialog, cx| {
                dialog.recent = match recent {
                    Ok(scan) => {
                        dialog.scan_warning = scan.t3_error;
                        RecentState::Ready(scan.directories)
                    }
                    Err(error) => RecentState::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(feature = "native-dialogs")]
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(crate::tr!("sidebar.select_project").into_owned().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(path) = paths.pop()
            {
                let _ = this.update_in(cx, |dialog, window, cx| {
                    dialog.create_draft(path, window, cx);
                });
            }
        })
        .detach();
    }

    /// Send the typed text to the host as-is. Whether it is absolute and whether
    /// it is a directory are facts about the host's filesystem and path rules,
    /// so the host decides and its answer is what the user reads. The path is
    /// never canonicalized or otherwise touched here.
    fn open_typed_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.path_input.read(cx).value().trim().to_owned();
        if typed.is_empty() {
            self.error = Some(crate::tr!("sidebar.path_required").into_owned());
            cx.notify();
            return;
        }
        self.create_draft(PathBuf::from(typed), window, cx);
    }

    fn create_draft(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let create = self
            .store
            .update(cx, |store, cx| store.create_project(path.clone(), cx));
        cx.spawn_in(window, async move |this, cx| {
            let project_id = match create.await {
                Ok(CommandResponse::ProjectId(Some(project_id))) => project_id,
                Ok(other) => {
                    let _ = this.update_in(cx, |dialog, _, cx| {
                        dialog.fail(format!("unexpected create-project response: {other:?}"), cx);
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update_in(cx, |dialog, _, cx| {
                        dialog.fail(error.message, cx);
                    });
                    return;
                }
            };
            let _ = this.update_in(cx, |dialog, window, cx| {
                dialog.store.update(cx, |store, cx| {
                    store.start_draft(project_id, path, cx);
                });
                window.close_dialog(cx);
            });
        })
        .detach();
    }

    fn fail(&mut self, reason: String, cx: &mut Context<Self>) {
        self.busy = false;
        self.error = Some(crate::tr!("sidebar.path_rejected", reason = reason).into_owned());
        cx.notify();
    }

    fn choose_recent(&mut self, recent: RecentDir, _window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.error = None;
        self.confirmation = Some(RecentConfirmation {
            recent: recent.clone(),
            choice: ImportChoice::Checking,
        });
        let inspect = self.store.update(cx, |store, cx| {
            store.inspect_t3_project(recent.path.clone(), cx)
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = inspect.await;
            let _ = this.update(cx, |dialog, cx| {
                let Some(confirmation) = &mut dialog.confirmation else {
                    return;
                };
                if confirmation.recent.path != recent.path
                    || !matches!(confirmation.choice, ImportChoice::Checking)
                {
                    return;
                }
                confirmation.choice = match result {
                    Ok(Some(history)) => ImportChoice::T3 {
                        history,
                        profiles: BTreeMap::new(),
                    },
                    Ok(None) => ImportChoice::Native,
                    Err(error) => ImportChoice::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel_recent(&mut self, cx: &mut Context<Self>) {
        self.confirmation = None;
        self.error = None;
        cx.notify();
    }

    fn decline_recent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirmation) = &mut self.confirmation else {
            return;
        };
        match confirmation.choice {
            ImportChoice::T3 { .. } | ImportChoice::Failed(_) => {
                confirmation.choice = ImportChoice::Native;
                self.error = None;
                cx.notify();
            }
            ImportChoice::Native => {
                let path = confirmation.recent.path.clone();
                self.create_draft(path, window, cx);
            }
            ImportChoice::Checking => {}
        }
    }

    fn confirm_recent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirmation) = self.confirmation.clone() else {
            return;
        };
        let profiles = match confirmation.choice {
            ImportChoice::T3 { history, profiles } => {
                if history
                    .profiles
                    .iter()
                    .any(|profile| !profiles.contains_key(&profile.id))
                {
                    return;
                }
                Some(profiles)
            }
            ImportChoice::Native => None,
            _ => return,
        };
        self.begin_import(confirmation.recent, profiles, window, cx);
    }

    fn begin_import(
        &mut self,
        recent: RecentDir,
        profiles: Option<BTreeMap<String, String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        self.error = None;
        let path = recent.path.clone();
        let create = self
            .store
            .update(cx, |store, cx| store.create_project(path, cx));
        let threads = recent.threads;
        let store = self.store.clone();
        cx.spawn_in(window, async move |this, cx| {
            let project_id = match create.await {
                Ok(CommandResponse::ProjectId(Some(project_id))) => project_id,
                Ok(other) => {
                    let _ = this.update_in(cx, |dialog, _, cx| {
                        dialog.fail(format!("unexpected create-project response: {other:?}"), cx);
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update_in(cx, |dialog, _, cx| dialog.fail(error.message, cx));
                    return;
                }
            };
            // Subscribe before starting: an import short enough to finish
            // before the start reply lands is only recoverable through the
            // retained status snapshot.
            let import = store.update(cx, |store, cx| {
                store.watch_external_import(&project_id);
                match profiles {
                    Some(profiles) => store.start_t3_import(&project_id, profiles, cx),
                    None => store.start_external_import(&project_id, threads, cx),
                }
            });
            let started = import.await;
            if !matches!(started, Ok(CommandResponse::ExternalImportStarted(true))) {
                store.update(cx, |store, _cx| {
                    store.unwatch_external_import(&project_id);
                });
                let reason = match started {
                    Err(error) => error.message,
                    _ => crate::tr!("sidebar.import_refused").into_owned(),
                };
                let _ = this.update_in(cx, |dialog, _, cx| {
                    dialog.busy = false;
                    dialog.error =
                        Some(crate::tr!("sidebar.import_failed", reason = reason).into_owned());
                    cx.notify();
                });
                return;
            }
            let _ = this.update_in(cx, |dialog, window, cx| {
                window.close_dialog(cx);
                let store = dialog.store.clone();
                let progress = cx.new(|cx| ImportProgress::new(store, project_id, cx));
                let content = progress.clone();
                window.open_dialog(cx, move |builder, _, cx| {
                    let progress_content = content.clone();
                    builder
                        .w(px(480.))
                        .rounded(crate::material::radius_overlay())
                        .bg(cx.theme().popover)
                        .border_1()
                        .border_color(cx.theme().border)
                        .shadow_xl()
                        .title(crate::tr!("sidebar.importing").into_owned())
                        .close_button(false)
                        .overlay_closable(false)
                        .keyboard(false)
                        .content(move |content_el, _, _| content_el.child(progress_content.clone()))
                });
            });
        })
        .detach();
    }

    fn render_confirmation(
        &self,
        confirmation: RecentConfirmation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (message, can_accept, can_decline) = match &confirmation.choice {
            ImportChoice::Checking => (
                crate::tr!("sidebar.import_checking").into_owned(),
                false,
                false,
            ),
            ImportChoice::T3 { history, profiles } => (
                crate::tr!("sidebar.import_t3_confirm", name = history.title.clone()).into_owned(),
                history
                    .profiles
                    .iter()
                    .all(|profile| profiles.contains_key(&profile.id)),
                true,
            ),
            ImportChoice::Native => (
                crate::tr!(
                    "sidebar.import_native_confirm",
                    tools = tool_counts(&confirmation.recent.threads)
                )
                .into_owned(),
                true,
                true,
            ),
            ImportChoice::Failed(error) => (
                crate::tr!("sidebar.import_t3_check_failed", reason = error.clone()).into_owned(),
                false,
                true,
            ),
        };
        let mut content = v_flex()
            .gap_3()
            .child(
                div()
                    .text_size(px(13.))
                    .font_semibold()
                    .child(directory_name(&confirmation.recent.path)),
            )
            .child(div().text_size(px(13.)).child(message));
        if let ImportChoice::T3 { history, profiles } = &confirmation.choice {
            let settings = self.store.read(cx).settings();
            for source in &history.profiles {
                let mut choices = h_flex().gap_2().flex_wrap();
                for profile in settings.profiles_for_kind(source.provider) {
                    let source_id = source.id.clone();
                    let selected = profiles.get(&source.id) == Some(&profile.id);
                    choices = choices.child(
                        Button::new(format!("import-profile-{}-{}", source.id, profile.id))
                            .label(settings.profile_display_name(&profile.id))
                            .when(selected, |button| button.primary())
                            .disabled(self.busy)
                            .on_click(cx.listener(move |dialog, _, _, cx| {
                                if let Some(RecentConfirmation {
                                    choice: ImportChoice::T3 { profiles, .. },
                                    ..
                                }) = &mut dialog.confirmation
                                {
                                    profiles.insert(source_id.clone(), profile.id.clone());
                                    cx.notify();
                                }
                            })),
                    );
                }
                content = content.child(
                    v_flex()
                        .gap_2()
                        .child(div().text_size(px(12.)).child(crate::tr!(
                            "sidebar.import_profile",
                            name = source.id.clone()
                        )))
                        .child(choices),
                );
            }
        }
        content
            .when_some(self.error.clone(), |column, error| {
                column.child(
                    div()
                        .text_size(px(12.))
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                DialogActions::new()
                    .child(
                        Button::new("import-cancel")
                            .label(crate::tr!("sidebar.cancel"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|dialog, _, _, cx| dialog.cancel_recent(cx))),
                    )
                    .child(
                        Button::new("import-no")
                            .label(crate::tr!("sidebar.import_no"))
                            .disabled(self.busy || !can_decline)
                            .on_click(cx.listener(|dialog, _, window, cx| {
                                dialog.decline_recent(window, cx)
                            })),
                    )
                    .child(
                        Button::new("import-yes")
                            .primary()
                            .label(crate::tr!("sidebar.import_yes"))
                            .disabled(self.busy || !can_accept)
                            .on_click(cx.listener(|dialog, _, window, cx| {
                                dialog.confirm_recent(window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_recent(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        match &self.recent {
            RecentState::Loading => v_flex()
                .gap_3()
                .py_4()
                .text_size(px(13.))
                .text_color(cx.theme().muted_foreground)
                .child(crate::tr!("sidebar.recent_loading"))
                .child(Progress::new("recent-directories-loading").loading(true))
                .into_any_element(),
            RecentState::Failed(error) => div()
                .py_4()
                .text_size(px(13.))
                .text_color(cx.theme().danger)
                .child(crate::tr!("sidebar.recent_failed", reason = error.clone()).into_owned())
                .into_any_element(),
            RecentState::Ready(recent) if recent.is_empty() => div()
                .py_4()
                .text_size(px(13.))
                .text_color(cx.theme().muted_foreground)
                .child(crate::tr!("sidebar.recent_empty"))
                .into_any_element(),
            RecentState::Ready(recent) => {
                // Keep the viewport and the flex column separate. Putting the
                // max height on the column itself lets flexbox shrink every row
                // until there is no overflow left for the wheel to scroll.
                let mut rows = v_flex().w_full().gap_1();
                for (index, recent) in recent.iter().take(RECENT_LIMIT).enumerate() {
                    let selected = recent.clone();
                    let name = directory_name(&recent.path);
                    let accessible_name =
                        crate::tr!("sidebar.open_recent", name = name.clone()).into_owned();
                    let path = middle_truncate(&recent.path, 76);
                    let ago = humanize_ago(now_secs().saturating_sub(recent.last_active_ms / 1000));
                    let counts = if recent.source_counts.is_empty() {
                        tool_counts(&recent.threads)
                    } else {
                        format_tool_counts(&recent.source_counts)
                    };
                    rows = rows.child(
                        crate::material::accessible_clickable(
                            v_flex(),
                            format!("recent-directory-{index}"),
                            Role::Button,
                            accessible_name,
                            cx,
                        )
                        .flex_none()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .rounded(crate::material::radius_card())
                        .text_size(px(13.))
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().list_hover))
                        .on_click(cx.listener(move |dialog, _, window, cx| {
                            dialog.choose_recent(selected.clone(), window, cx);
                        }))
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .gap_3()
                                .child(div().font_bold().child(name))
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(ago),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .child(path),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .child(counts),
                        ),
                    );
                }
                let visible_rows = recent.len().min(RECENT_LIMIT) as f32;
                let viewport_height = fit_viewport(
                    (visible_rows * RECENT_ROW_HEIGHT_ESTIMATE).min(RECENT_VIEWPORT_MAX_HEIGHT),
                    window.viewport_size().height - px(RECENT_VIEWPORT_CHROME),
                );
                div()
                    .id("recent-directory-list")
                    .w_full()
                    .h(viewport_height)
                    .overflow_y_scrollbar()
                    .child(div().size_full().child(rows))
                    .into_any_element()
            }
        }
    }
}

impl Render for AddProjectDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(confirmation) = self.confirmation.clone() {
            return self.render_confirmation(confirmation, cx);
        }
        let host = self.store.read(cx).remote_host_name().map(str::to_owned);
        let recent_label = match &host {
            Some(host) => crate::tr!("sidebar.recent_activity_host", host = host).into_owned(),
            None => crate::tr!("sidebar.recent_activity").into_owned(),
        };
        let path_hint = host
            .as_ref()
            .map(|host| crate::tr!("sidebar.host_path_hint", host = host).into_owned());
        let can_browse = self.can_browse(cx);
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_size(px(13.)).font_semibold().child(recent_label))
                    .when_some(self.scan_warning.clone(), |column, reason| {
                        column.child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().danger)
                                .child(crate::tr!(
                                    "sidebar.import_t3_scan_failed",
                                    reason = reason
                                )),
                        )
                    })
                    .child(self.render_recent(window, cx)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(
                                Input::new(&self.path_input)
                                    .flex_1()
                                    .rounded(crate::material::radius_input()),
                            )
                            .when(can_browse, |row| {
                                row.child(
                                    Button::new("browse-project-directory")
                                        .rounded(crate::material::radius_button())
                                        .label(crate::tr!("sidebar.browse"))
                                        .on_click(cx.listener(|dialog, _, window, cx| {
                                            dialog.browse_clicked(window, cx);
                                        })),
                                )
                            }),
                    )
                    .when_some(path_hint, |column, hint| {
                        column.child(
                            div()
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .child(hint),
                        )
                    })
                    .when_some(self.error.clone(), |column, error| {
                        column.child(
                            div()
                                .text_size(px(11.))
                                .text_color(cx.theme().danger)
                                .child(error),
                        )
                    }),
            )
            .child(render_add_footer(&cx.entity(), window, cx))
            .into_any_element()
    }
}

impl AddProjectDialog {
    #[cfg(feature = "native-dialogs")]
    fn browse_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browse(window, cx);
    }

    /// Unreachable: the button is only rendered when `can_browse`, which is
    /// false without the feature.
    #[cfg(not(feature = "native-dialogs"))]
    fn browse_clicked(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
}

/// Renders the host's replicated import status. It owns no progress state of
/// its own, so a completion that arrived before this view existed still shows
/// up: the subscription snapshot carries the retained latest run.
struct ImportProgress {
    store: Entity<WorkspaceStore>,
    project_id: String,
    _subscription: gpui::Subscription,
}

impl ImportProgress {
    fn new(store: Entity<WorkspaceStore>, project_id: String, cx: &mut Context<Self>) -> Self {
        Self {
            _subscription: observe_store_topics(&store, &[TopicKind::ExternalImport], cx),
            store,
            project_id,
        }
    }
}

impl Render for ImportProgress {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self
            .store
            .read(cx)
            .external_import_status(&self.project_id)
            .map(|status| status.state.clone());
        // The "n of N" line describes a run still in flight; the summary below
        // replaces it once the host reports the outcome.
        let running = match &state {
            Some(ExternalImportState::Progress { done, total, tool }) => {
                Some((*done, *total, tool.clone()))
            }
            _ => None,
        };
        let failure = match &state {
            Some(ExternalImportState::Failed { message }) => Some(message.clone()),
            _ => None,
        };
        let finished =
            failure.is_some() || matches!(state, Some(ExternalImportState::Finished { .. }));
        let summary = match state {
            Some(ExternalImportState::Finished { imported, skipped }) => Some((imported, skipped)),
            _ => None,
        };
        // A run with nothing to import is complete the moment it starts; a
        // status that has not arrived yet is not.
        let percent = match running {
            Some((_, 0, _)) | None if summary.is_none() => 0.0,
            Some((done, total, _)) if total > 0 => done as f32 * 100.0 / total as f32,
            _ => 100.0,
        };
        let project_id = self.project_id.clone();
        let store = self.store.clone();
        v_flex()
            .gap_3()
            .py_2()
            .child(Progress::new("external-import-progress").value(percent))
            .when_some(running, |column, (done, total, tool)| {
                column.child(
                    div()
                        .text_size(px(13.))
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::tr!(
                            "sidebar.import_progress",
                            done = done,
                            total = total,
                            tool = tool
                        )),
                )
            })
            .when_some(summary, |column, (imported, skipped)| {
                column.child(
                    div()
                        .text_size(px(13.))
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(crate::tr!(
                            "sidebar.import_summary",
                            imported = imported,
                            skipped = skipped
                        )),
                )
            })
            .when_some(failure, |column, error| {
                column.child(
                    div()
                        .text_size(px(13.))
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .when(finished, |column| {
                column.child(
                    h_flex().w_full().justify_end().child(
                        Button::new("external-import-ok")
                            .rounded(crate::material::radius_button())
                            .primary()
                            .label(crate::tr!("sidebar.import_ok"))
                            .on_click(move |_, window, cx| {
                                store.update(cx, |store, _cx| {
                                    store.unwatch_external_import(&project_id);
                                });
                                window.close_dialog(cx);
                            }),
                    ),
                )
            })
    }
}

fn render_add_footer(
    dialog: &Entity<AddProjectDialog>,
    _window: &mut Window,
    _cx: &mut App,
) -> AnyElement {
    let open = dialog.clone();
    DialogActions::new()
        .child(
            Button::new("add-project-cancel")
                .rounded(crate::material::radius_button())
                .label(crate::tr!("sidebar.cancel"))
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                }),
        )
        .child(
            Button::new("add-project-open")
                .rounded(crate::material::radius_button())
                .primary()
                .label(crate::tr!("sidebar.open"))
                .on_click(move |_, window, cx| {
                    open.update(cx, |dialog, cx| dialog.open_typed_path(window, cx));
                }),
        )
        .into_any_element()
}

fn directory_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

fn middle_truncate(path: &Path, max_chars: usize) -> String {
    let text = path.display().to_string();
    let chars: Vec<_> = text.chars().collect();
    if chars.len() <= max_chars {
        return text;
    }
    let left = (max_chars - 1) / 2;
    let right = max_chars - left - 1;
    format!(
        "{}…{}",
        chars[..left].iter().collect::<String>(),
        chars[chars.len() - right..].iter().collect::<String>()
    )
}

fn tool_counts(threads: &[ExternalThread]) -> String {
    let mut counts = HashMap::new();
    for thread in threads {
        *counts.entry(thread.source).or_insert(0_usize) += 1;
    }
    format_tool_counts(&counts)
}

fn format_tool_counts(counts: &HashMap<SourceTool, usize>) -> String {
    [
        SourceTool::T3Code,
        SourceTool::ClaudeCode,
        SourceTool::ClaudeDesktop,
        SourceTool::CodexCli,
        SourceTool::CodexDesktop,
    ]
    .into_iter()
    .filter_map(|source| {
        counts
            .get(&source)
            .map(|count| format!("{} ×{count}", source.display_name()))
    })
    .collect::<Vec<_>>()
    .join(" · ")
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};
    use tcode_runtime::pipe::{HostServices, spawn_host};
    use tcode_services::store::SessionStore;

    use super::*;
    use crate::store::WorkspaceAttachment;

    /// A remote client types a path that means nothing on its own machine. The
    /// answer — accept or reject, and why — comes from the host's filesystem,
    /// and the client shows the host's words rather than judging the string
    /// against its own path rules.
    #[gpui::test]
    fn a_remote_project_root_is_judged_by_the_host(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "tcode-add-project-remote-{}",
            tcode_services::store::now_millis()
        ));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let host = spawn_host(
            SessionStore::open_at(root.join("data")).unwrap(),
            HostServices::default(),
        )
        .expect("spawn add-project test host");
        let store = cx.new(|cx| {
            WorkspaceStore::new_attached(
                host.link(),
                WorkspaceAttachment::Remote {
                    host_id: "build-box".into(),
                    host_name: "build-box".into(),
                },
                None,
                true,
                cx,
            )
        });
        // The dialog closes itself on success, which routes through the
        // window's overlay root — so mount it the way the app does.
        let built = std::rc::Rc::new(std::cell::RefCell::new(None));
        let capture = built.clone();
        let store_for_view = store.clone();
        let (_root, cx) = cx.add_window_view(move |window, cx| {
            let dialog = cx.new(|cx| AddProjectDialog::new(store_for_view.clone(), window, cx));
            *capture.borrow_mut() = Some(dialog.clone());
            crate::overlay::OverlayHost::new(dialog, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        let dialog = built.borrow().clone().expect("dialog was built");

        let type_and_open = |cx: &mut VisualTestContext, path: &str| {
            let path = path.to_owned();
            cx.update(|window, cx| {
                dialog.update(cx, |dialog, cx| {
                    dialog
                        .path_input
                        .update(cx, |input, cx| input.set_value(path.clone(), window, cx));
                    dialog.open_typed_path(window, cx);
                });
            });
            cx.run_until_parked();
        };

        // A Windows-style path on a Unix host (or a nonexistent one on Windows):
        // rejected there, not here.
        type_and_open(cx, r"C:\Users\dev\src");
        let error = dialog.read_with(cx, |dialog, _| dialog.error.clone());
        assert!(
            error.is_some_and(|error| error.contains(r"C:\Users\dev\src")),
            "the dialog must show the host's reason for refusing the path"
        );
        assert_eq!(
            smol::block_on(host.update_state_for_test(|state, _| state.projects.len()))
                .expect("read projects"),
            0,
            "a refused root must not create a project"
        );

        // A directory that exists on the host is accepted even though this
        // client never looked at it.
        type_and_open(cx, workspace.to_str().unwrap());
        assert_eq!(dialog.read_with(cx, |dialog, _| dialog.error.clone()), None);
        assert_eq!(
            smol::block_on(host.update_state_for_test(|state, _| {
                state
                    .projects
                    .iter()
                    .map(|project| project.root.clone())
                    .collect::<Vec<_>>()
            }))
            .expect("read projects"),
            vec![workspace.clone()]
        );

        host.shutdown_blocking().expect("stop host");
        let _ = std::fs::remove_dir_all(root);
    }
    #[gpui::test]
    fn recent_imports_confirm_sources_before_creating_a_project(cx: &mut TestAppContext) {
        use tcode_protocol::{
            ClientPayload, HostMessage, Query, QueryResponse, T3ImportProfile, T3ProjectHistory,
        };
        cx.update(crate::theme::init);
        let (to_host, requests) = async_channel::unbounded();
        let (replies, from_host) = async_channel::unbounded();
        let link = tcode_client::HostLink::new(to_host, from_host);
        let pump_link = link.clone();
        let executor = cx.background_executor.clone();
        let _pump = cx.background_executor.spawn(async move {
            pump_link
                .pump_with_timer(|| executor.timer(std::time::Duration::from_millis(25)))
                .await;
        });
        let store = cx.new(|cx| {
            WorkspaceStore::new_attached(link, WorkspaceAttachment::Local, None, false, cx)
        });
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let dialog = cx.new(|cx| AddProjectDialog::new(store, window, cx));
            view = Some(dialog.clone());
            crate::overlay::OverlayHost::new(dialog, window, cx)
        });
        let dialog = view.unwrap();
        while requests.try_recv().is_ok() {}
        for width in [393., 1000.] {
            cx.simulate_resize(gpui::size(px(width), px(800.)));
            for choice in ["cancel_check", "t3", "native", "failed"] {
                let recent = RecentDir {
                    path: PathBuf::from("/host/project"),
                    last_active_ms: 0,
                    source_counts: HashMap::new(),
                    threads: vec![ExternalThread {
                        source: SourceTool::CodexCli,
                        file: PathBuf::from("/host/native.jsonl"),
                        external_id: "codex:native".into(),
                        title_hint: None,
                        last_active_ms: 0,
                    }],
                };
                cx.update(|window, cx| {
                    dialog.update(cx, |dialog, cx| {
                        dialog.choose_recent(recent.clone(), window, cx)
                    })
                });
                cx.run_until_parked();
                let message =
                    tcode_protocol::decode_client_line(&requests.try_recv().unwrap()).unwrap();
                assert_eq!(
                    message.payload,
                    ClientPayload::Query(Query::InspectT3Project { root: recent.path })
                );
                assert!(
                    requests.is_empty(),
                    "selection must only inspect, never create or import"
                );
                if choice == "cancel_check" {
                    cx.update(|_, cx| dialog.update(cx, |dialog, cx| dialog.cancel_recent(cx)));
                }
                let result = match choice {
                    "native" => Ok(QueryResponse::T3Project(None)),
                    "failed" => Err(tcode_protocol::ProtocolError {
                        code: "t3_inspection_failed".into(),
                        message: "Unreadable T3 history".into(),
                    }),
                    _ => Ok(QueryResponse::T3Project(Some(T3ProjectHistory {
                        title: "T3 title".into(),
                        profiles: vec![T3ImportProfile {
                            id: "Custom".into(),
                            provider: agent::ProviderKind::Codex,
                        }],
                    }))),
                };
                replies
                    .try_send(
                        tcode_protocol::encode_line(&HostMessage::QueryResult {
                            id: message.id,
                            result,
                        })
                        .unwrap(),
                    )
                    .unwrap();
                cx.run_until_parked();
                assert!(requests.is_empty(), "inspection must not start a write");
                if choice == "cancel_check" {
                    assert!(dialog.read_with(cx, |dialog, _| dialog.confirmation.is_none()));
                    continue;
                }
                if choice == "t3" {
                    cx.update(|window, cx| {
                        dialog.update(cx, |dialog, cx| dialog.confirm_recent(window, cx))
                    });
                    cx.run_until_parked();
                    assert!(
                        requests.is_empty(),
                        "custom T3 instances need an explicit profile choice"
                    );
                }
                if matches!(choice, "t3" | "failed") {
                    cx.update(|window, cx| {
                        dialog.update(cx, |dialog, cx| dialog.decline_recent(window, cx))
                    });
                }
                assert!(dialog.read_with(cx, |dialog, _| matches!(
                    dialog.confirmation.as_ref().unwrap().choice,
                    ImportChoice::Native
                )));
                cx.update(|_, cx| dialog.update(cx, |dialog, cx| dialog.cancel_recent(cx)));
                cx.run_until_parked();
                assert!(
                    requests.is_empty(),
                    "No to T3 and Cancel must not create a project"
                );
            }
        }
    }
}
