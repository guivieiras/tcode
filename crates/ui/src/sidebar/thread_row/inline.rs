//! The original arrangement: status glyphs and the project inline with the text.
use super::*;

pub(super) fn wide_height(kind: Kind) -> f32 {
    if kind == Kind::Grouped {
        crate::material::selectable_row_height(false)
    } else {
        FLAT_ROW_INNER_HEIGHT
    }
}

pub(super) fn render(row: Row<'_>, cx: &mut Context<SessionsSidebar>) -> gpui::AnyElement {
    let title = row.title(cx);
    let pin = row.pin(cx);
    if row.kind == Kind::Compact {
        let project = row.project(13., None, cx);
        let time = row.time(13., cx);
        let metadata = row.metadata(project, Some(time), cx);
        let artwork = row
            .project_name
            .as_ref()
            .and(row.meta.project_id.as_deref())
            .and_then(|id| row.sidebar.store.read(cx).project(id).cloned());
        return h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap_3()
            .child(compact_waiting_glyph(row.state, artwork.as_ref(), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(design(2.))
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap(design(6.))
                            .child(title)
                            .children(pin),
                    )
                    .child(metadata.w_full()),
            )
            .into_any_element();
    }
    let time = row.time(11., cx);
    if row.kind == Kind::Grouped {
        return h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(design(6.))
            .children(SessionsSidebar::thread_attention_badge(row.state, cx))
            .children(row.worktree(cx))
            .child(title)
            .children(pin)
            .children(row.pull_request(cx))
            .child(time)
            .into_any_element();
    }
    let attention = (row.sidebar.renaming_input(&row.meta.id).is_none())
        .then(|| SessionsSidebar::thread_status_label(row.state, row.working, cx))
        .flatten()
        .filter(|_| row.state.waiting() || row.state.failed);
    let project = row.project(11., Some(12.), cx);
    let metadata = row.metadata(project, None, cx);
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design(2.))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(design(6.))
                .when_some(attention, |line, (color, _, _)| {
                    line.child(div().flex_none().size(design(6.)).rounded_full().bg(color))
                })
                .child(title)
                .children(pin)
                .child(time),
        )
        .child(metadata.w_full().child(div().flex_1()))
        .into_any_element()
}
