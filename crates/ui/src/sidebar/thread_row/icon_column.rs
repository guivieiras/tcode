//! A glyph column beside the title and project metadata.
use super::*;

pub(super) const HEIGHT: f32 = 48.;

pub(super) fn render(mut row: Row<'_>, cx: &mut Context<SessionsSidebar>) -> gpui::AnyElement {
    if row.project_name.is_none() {
        row.project_name = row
            .meta
            .project_id
            .as_deref()
            .and_then(|id| row.sidebar.store.read(cx).project(id))
            .map(|project| crate::project_icon::label(project).into());
    }
    let title = row.title(cx);
    let pin = row.pin(cx);
    let time = row.time(11., cx);
    let project = row.project(11., None, cx);
    let icon = div()
        .flex_none()
        .w(design(20.))
        .ml(design(-2.))
        .h(design(20.))
        .flex()
        .items_center()
        .justify_center()
        .child(row.project_icon(18., cx));
    let metadata = row.metadata(project, None, cx);
    let text = v_flex()
        .flex_1()
        .min_w_0()
        .gap(design(2.))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(design(8.))
                .child(title)
                .children(pin)
                .child(time),
        )
        .child(metadata.w_full().child(div().flex_1()));
    h_flex()
        .w_full()
        .min_w_0()
        .items_start()
        .gap(design(8.))
        .child(icon)
        .child(text)
        .into_any_element()
}
