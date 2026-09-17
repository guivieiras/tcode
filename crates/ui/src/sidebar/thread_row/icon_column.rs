//! A glyph column beside the title and larger project metadata.
use super::*;

pub(super) const HEIGHT: f32 = 60.;

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
    let time = row.time(14., cx);
    let project = row.project(16., None, cx);
    let icon = div()
        .flex_none()
        .w(px(28.))
        .self_stretch()
        .flex()
        .items_center()
        .justify_center()
        .child(row.project_icon(18., cx));
    if row.kind == Kind::Compact {
        let metadata = row.metadata(project, Some(time), cx);
        return h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(10.))
            .child(icon)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap(px(6.))
                            .child(title)
                            .children(pin),
                    )
                    .child(metadata.w_full()),
            )
            .into_any_element();
    }
    let has_project = project.is_some();
    let metadata = row.metadata(project, None, cx);
    let text = v_flex()
        .flex_1()
        .min_w_0()
        .gap(px(4.))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(px(8.))
                .child(title)
                .children(pin)
                .child(time),
        )
        .child(
            metadata
                .w_full()
                .when(!has_project, |line| line.child(div().flex_1())),
        );
    h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap(px(10.))
        .child(icon)
        .child(text)
        .into_any_element()
}
