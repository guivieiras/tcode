//! Built-in row presentations. Layout files arrange controls; this module owns
//! their shared metadata and disclosure behavior. Session state stays in the sidebar.

mod icon_column;
mod inline;

use super::*;
use tcode_client::host::ThreadAppearance;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Grouped,
    Recent,
    Compact,
}

pub(super) struct Row<'a> {
    pub sidebar: &'a SessionsSidebar,
    pub meta: &'a SessionMeta,
    pub state: &'a ThreadRowState,
    pub working: bool,
    pub project_name: Option<SharedString>,
    pub kind: Kind,
}

pub(super) fn render(row: Row<'_>, cx: &mut Context<SessionsSidebar>) -> gpui::AnyElement {
    if row.kind == Kind::Compact {
        return inline::render(row, cx);
    }
    match row.sidebar.store.read(cx).thread_appearance() {
        ThreadAppearance::Inline => inline::render(row, cx),
        ThreadAppearance::IconColumn => icon_column::render(row, cx),
    }
}

/// The virtual list and the row frame must agree on the occupied height.
pub(super) fn wide_height(appearance: ThreadAppearance, kind: Kind, child: bool) -> f32 {
    match appearance {
        ThreadAppearance::Inline => inline::wide_height(kind, child),
        ThreadAppearance::IconColumn => icon_column::HEIGHT,
    }
}

impl Row<'_> {
    fn title(&self, cx: &mut Context<SessionsSidebar>) -> gpui::AnyElement {
        self.sidebar.thread_title_or_input(
            self.meta,
            self.state,
            self.kind != Kind::Grouped && !self.state.is_child,
            cx,
        )
    }

    fn time(&self, font_size: f32, cx: &mut Context<SessionsSidebar>) -> gpui::AnyElement {
        self.sidebar
            .render_thread_time(self.meta, self.state, self.working, font_size, cx)
            .into_any_element()
    }

    fn leading_icon(&self, size: f32, cx: &App) -> gpui::AnyElement {
        if self.state.is_child {
            div()
                .flex_none()
                .text_size(design(size))
                .text_color(cx.theme().muted_foreground)
                .child("↳")
                .into_any_element()
        } else {
            self.project_icon(size, cx)
        }
    }

    fn project_icon(&self, size: f32, cx: &App) -> gpui::AnyElement {
        self.sidebar
            .store
            .read(cx)
            .project(self.meta.project_id.as_deref().unwrap_or_default())
            .map(|project| crate::project_icon::artwork(project, size).into_any_element())
            .unwrap_or_else(|| {
                Icon::new(IconName::Folder)
                    .size(design(size))
                    .into_any_element()
            })
    }

    fn project(
        &self,
        font_size: f32,
        icon_size: Option<f32>,
        cx: &App,
    ) -> Option<gpui::AnyElement> {
        if self.state.is_child {
            return None;
        }
        let name = self.project_name.clone()?;
        Some(
            h_flex()
                .min_w_0()
                .gap(design(4.))
                .debug_selector({
                    let id = self.meta.id.clone();
                    move || format!("thread-project-{id}")
                })
                .when_some(icon_size, |line, size| {
                    line.child(self.project_icon(size, cx))
                })
                .child(
                    truncated_sidebar_label()
                        .text_size(design(font_size))
                        .line_height(design((font_size + 4.).max(18.)))
                        .debug_selector({
                            let name = name.clone();
                            move || format!("compact-project-{name}")
                        })
                        .child(name),
                )
                .into_any_element(),
        )
    }

    fn metadata(
        &self,
        project: Option<gpui::AnyElement>,
        time: Option<gpui::AnyElement>,
        cx: &mut Context<SessionsSidebar>,
    ) -> gpui::Div {
        let compact = self.kind == Kind::Compact;
        let has_project = project.is_some();
        let status = if compact || !self.state.waiting() {
            thread_status_line(self.state, self.working, cx)
        } else {
            None
        };
        let has_status = status.is_some();
        let mut line = h_flex()
            .min_w_0()
            .items_center()
            .gap(design(4.))
            .text_size(design(if compact { 13. } else { 11. }))
            .line_height(design(18.))
            .text_color(cx.theme().muted_foreground);
        if compact {
            if self
                .sidebar
                .store
                .read(cx)
                .session_has_pending_writes(&self.meta.id)
            {
                line = line.child(crate::tr!("sidebar.pending_write"));
            }
            if self.meta.parent_session_id.is_some() && !self.state.is_child {
                line = line
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .debug_selector(|| "compact-parent-unavailable".into())
                            .child(crate::tr!("sidebar.parent_unavailable")),
                    )
                    .when(has_project || has_status, |line| {
                        line.child(div().flex_none().child("·"))
                    });
            }
            line = line.children(project);
        } else {
            line = line
                .children(SessionsSidebar::thread_waiting_badge(self.state, cx))
                .when(self.state.waiting() && has_project, |line| {
                    line.child(div().flex_none().child("·"))
                })
                .children(project);
        }
        if has_project && has_status {
            line = line.child(div().flex_none().child("·"));
        }
        if let Some((label, color)) = status {
            line = line.child(div().flex_none().text_color(color).child(label));
        }
        line = line.when(self.state.is_worktree, |line| {
            line.child(
                Icon::empty()
                    .path("icons/git-branch.svg")
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            )
        });
        line.when_some(time, |line, time| {
            let has_metadata = has_project
                || self.state.is_worktree
                || has_status
                || (self.meta.parent_session_id.is_some() && !self.state.is_child)
                || self
                    .sidebar
                    .store
                    .read(cx)
                    .session_has_pending_writes(&self.meta.id);
            line.when(
                has_metadata && (!has_status || self.state.waiting()),
                |line| line.child(div().flex_none().child("·")),
            )
            .child(time)
        })
    }

    fn disclosure(&self, cx: &mut Context<SessionsSidebar>) -> Option<gpui::AnyElement> {
        if !self.state.has_direct_children() {
            return None;
        }
        let chevron = collapse_chevron(self.state.children_collapsed, cx);
        let badge = child_count_badge(
            &self.meta.id,
            self.state.direct_children,
            self.state.active_direct_children,
            cx,
        );
        if self.kind != Kind::Compact {
            return Some(
                h_flex()
                    .flex_none()
                    .gap(design(2.))
                    .child(chevron)
                    .child(badge)
                    .into_any_element(),
            );
        }
        let session_id = self.meta.id.clone();
        Some(
            crate::material::accessible_clickable(
                h_flex(),
                SharedString::from(format!("compact-children-{session_id}")),
                Role::Button,
                crate::tr!("sidebar.child_threads", count = self.state.direct_children)
                    .into_owned(),
                cx,
            )
            .debug_selector({
                let id = session_id.clone();
                move || format!("compact-children-{id}")
            })
            .aria_expanded(!self.state.children_collapsed)
            .flex_none()
            .min_w(design(44.))
            .h(design(44.))
            .justify_center()
            .gap(design(2.))
            .text_size(design(12.))
            .text_color(cx.theme().muted_foreground)
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if !this.collapsed_parents.remove(&session_id) {
                    this.collapsed_parents.insert(session_id.clone());
                }
                this.compact_model_dirty = true;
                cx.notify();
            }))
            .child(chevron)
            .child(self.state.direct_children.to_string())
            .into_any_element(),
        )
    }
}
