//! The original arrangement: inline glyphs and compact metadata.
use super::*;

pub(super) fn wide_height(kind: Kind, child: bool) -> f32 {
    if kind == Kind::Grouped || child {
        30.
    } else {
        48.
    }
}

pub(super) fn render(row: Row<'_>, cx: &mut Context<SessionsSidebar>) -> gpui::AnyElement {
    let title = row.title(cx);
    let disclosure = row.disclosure(cx);
    if row.kind == Kind::Compact {
        let project = row.project(13., None, cx);
        let time = row.time(13., cx);
        let metadata = row.metadata(project, Some(time), cx);
        return h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(12.))
            .child(compact_waiting_glyph(
                row.state,
                row.project_name.as_ref().and_then(|_| {
                    row.sidebar
                        .store
                        .read(cx)
                        .project(row.meta.project_id.as_deref().unwrap_or_default())
                }),
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(title)
                    .child(metadata.w_full()),
            )
            .children(disclosure)
            .into_any_element();
    }
    let time = row.time(11., cx);
    if row.kind == Kind::Grouped || row.state.is_child {
        let metadata = row.metadata(None, None, cx);
        return h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(8.))
            .when(row.state.waiting() || row.state.is_worktree, |line| {
                line.child(metadata)
            })
            .when(row.state.is_child, |line| {
                line.child(row.leading_icon(13., cx))
            })
            .child(title)
            .children(disclosure)
            .child(time)
            .into_any_element();
    }
    let project = row.project(11., Some(12.), cx);
    let metadata = row.metadata(project, None, cx);
    v_flex()
        .w_full()
        .min_w_0()
        .gap(px(2.))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(px(8.))
                .when(row.state.waiting() && !row.state.show_completed, |line| {
                    line.child(
                        div()
                            .flex_none()
                            .size(px(6.))
                            .rounded_full()
                            .bg(cx.theme().warning),
                    )
                })
                .child(title)
                .child(time),
        )
        .child(metadata.w_full().child(div().flex_1()).children(disclosure))
        .into_any_element()
}
