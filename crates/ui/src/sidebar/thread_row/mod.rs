//! Built-in thread row presentations. Layout files arrange a row's parts; this
//! module owns the parts they share. Thread state stays in the sidebar.

mod icon_column;
mod inline;

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// A row of the project view, under its project's header.
    Grouped,
    /// A row of the threads view, which names its project.
    Recent,
    Compact,
}

pub(super) struct Row<'a> {
    pub sidebar: &'a SessionsSidebar,
    pub meta: &'a SessionMeta,
    pub state: &'a ThreadRowState,
    pub working: bool,
    pub project_name: Option<SharedString>,
    /// The time the row shows: the one its list sorts it by.
    pub timestamp: u64,
    pub kind: Kind,
}

pub(super) fn render(row: Row<'_>, cx: &mut Context<SessionsSidebar>) -> gpui::AnyElement {
    match row.sidebar.store.read(cx).thread_appearance() {
        ThreadAppearance::Inline => inline::render(row, cx),
        ThreadAppearance::IconColumn => icon_column::render(row, cx),
    }
}

/// The virtual list and the row frame must agree on the occupied height.
pub(super) fn wide_height(appearance: ThreadAppearance, kind: Kind) -> f32 {
    match appearance {
        ThreadAppearance::Inline => inline::wide_height(kind),
        ThreadAppearance::IconColumn => icon_column::HEIGHT,
    }
}

impl Row<'_> {
    fn title(&self, cx: &mut Context<SessionsSidebar>) -> gpui::AnyElement {
        self.sidebar
            .thread_title_or_input(self.meta, self.state, self.kind != Kind::Grouped, cx)
    }

    /// A pinned row's pin; on a wide row's hover it becomes the Unpin button.
    fn pin(&self, cx: &mut Context<SessionsSidebar>) -> Option<gpui::AnyElement> {
        if self.kind != Kind::Compact {
            return self
                .sidebar
                .pin_glyph(self.meta, &self.state.row_key, cx)
                .map(IntoElement::into_any_element);
        }
        (self.meta.pinned_at.is_some() && !self.meta.is_settled()).then(|| {
            Icon::new(IconName::Pin)
                .size(px(14.))
                .flex_none()
                .text_color(cx.theme().muted_foreground)
                .into_any_element()
        })
    }

    fn worktree(&self, cx: &App) -> Option<gpui::AnyElement> {
        self.state.is_worktree.then(|| {
            Icon::empty()
                .path("icons/git-branch.svg")
                .xsmall()
                .text_color(cx.theme().muted_foreground)
                .into_any_element()
        })
    }

    fn pull_request(&self, cx: &mut Context<SessionsSidebar>) -> Option<gpui::AnyElement> {
        if self.kind == Kind::Compact {
            return crate::pull_requests::badge(
                &self.meta.pull_requests,
                self.meta.branch_pull_request.as_deref(),
                14.,
                cx,
            );
        }
        if self.sidebar.renaming_input(&self.meta.id).is_some() {
            return None;
        }
        crate::pull_requests::sidebar_badge(
            &self.meta.pull_requests,
            self.meta.branch_pull_request.as_deref(),
            &self.meta.id,
            self.sidebar.store.clone(),
            cx,
        )
    }

    fn time(&self, font_size: f32, cx: &mut Context<SessionsSidebar>) -> gpui::AnyElement {
        self.sidebar
            .render_thread_time(
                self.meta,
                self.state,
                self.working,
                self.timestamp,
                font_size,
                cx,
            )
            .into_any_element()
    }

    fn project_icon(&self, size: f32, cx: &App) -> gpui::AnyElement {
        self.sidebar
            .store
            .read(cx)
            .project(self.meta.project_id.as_deref().unwrap_or_default())
            .map(|project| crate::project_icon::artwork(project, size))
            .unwrap_or_else(|| {
                Icon::new(IconName::Folder)
                    .size(px(size))
                    .into_any_element()
            })
    }

    fn project(
        &self,
        font_size: f32,
        icon_size: Option<f32>,
        cx: &App,
    ) -> Option<gpui::AnyElement> {
        let name = self.project_name.clone()?;
        Some(
            h_flex()
                .when(self.kind != Kind::Compact, |line| line.flex_1())
                .min_w_0()
                .gap(px(4.))
                .debug_selector({
                    let id = self.meta.id.clone();
                    move || format!("thread-project-{id}")
                })
                .when_some(icon_size, |line, size| {
                    line.child(self.project_icon(size, cx))
                })
                .child(
                    truncated_sidebar_label()
                        .text_size(px(font_size))
                        .line_height(px((font_size + 4.).max(18.)))
                        .debug_selector({
                            let name = name.clone();
                            move || format!("compact-project-{name}")
                        })
                        .child(name),
                )
                .into_any_element(),
        )
    }

    /// The row's metadata line: what needs the user, the project, the worktree
    /// and the pull request, then `time` where the layout puts it there.
    fn metadata(
        &self,
        project: Option<gpui::AnyElement>,
        time: Option<gpui::AnyElement>,
        cx: &mut Context<SessionsSidebar>,
    ) -> gpui::Div {
        let compact = self.kind == Kind::Compact;
        let separator = || div().flex_none().child("·");
        let mut line = h_flex()
            .min_w_0()
            .items_center()
            .gap(px(4.))
            .text_size(px(if compact { 13. } else { 11. }))
            .line_height(px(18.))
            .text_color(cx.theme().muted_foreground);
        let status = if compact {
            compact_status_line(self.state, self.working, cx)
                .map(|(label, color)| div().flex_none().text_color(color).child(label))
                .map(IntoElement::into_any_element)
        } else {
            SessionsSidebar::thread_attention_badge(self.state, cx)
        };
        let mut has_metadata = false;
        let worktree = self.worktree(cx);
        // The wide rows lead with what needs the user; the phone names the
        // project first, as its list does.
        let (first, second) = if compact {
            (project, status)
        } else {
            (status, project)
        };
        for part in [first, second].into_iter().flatten() {
            line = line
                .when(has_metadata, |line| line.child(separator()))
                .child(part);
            has_metadata = true;
        }
        if let Some(worktree) = worktree {
            line = line.child(worktree);
            has_metadata = true;
        }
        if let Some(badge) = self.pull_request(cx) {
            line = line
                .when(compact && has_metadata, |line| line.child(separator()))
                .child(badge);
            has_metadata = true;
        }
        line.when_some(time, |line, time| {
            line.when(has_metadata, |line| line.child(separator()))
                .child(time)
        })
    }
}
