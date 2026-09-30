//! A glyph column beside the title and project metadata.
use super::*;

pub(super) const HEIGHT: f32 = 48.;

pub(super) fn render(mut row: Row<'_>, cx: &mut Context<SessionsSidebar>) -> gpui::AnyElement {
    if row.project_name.is_none() {
        row.project_name = row
            .meta
            .project_id
            .as_ref()
            .and_then(|id| row.sidebar.store.read(cx).project_summary(id))
            .map(|(name, _)| name.into());
    }
    let title = row.title(cx);
    let time = row.time(11., cx);
    let project = row.project(11., None, cx);
    let disclosure = row.disclosure(cx);
    let icon = div()
        .flex_none()
        .w(px(20.))
        .ml(px(-2.))
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .child(row.leading_icon(18., cx));
    let metadata = row.metadata(project, None, cx);
    let text = v_flex()
        .flex_1()
        .min_w_0()
        .gap(px(2.))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(px(8.))
                .child(title)
                .child(time),
        )
        .child(metadata.w_full().child(div().flex_1()).children(disclosure));
    h_flex()
        .w_full()
        .min_w_0()
        .items_start()
        .gap(px(8.))
        .child(icon)
        .child(text)
        .into_any_element()
}
