//! Client-local UI scale. Design dimensions resolve through GPUI's window rem size.

use gpui::{App, Global, KeyBinding, Window, actions, px};

use crate::remote::ClientAttachment;

pub(crate) const LEVELS: &[u16] = &[75, 90, 100, 110, 125, 150, 175, 200];

actions!(zoom, [ZoomIn, ZoomOut, ResetZoom]);

#[derive(Clone, Copy)]
pub(crate) struct Zoom(u16);
impl Global for Zoom {}

pub(crate) fn restore(percent: Option<u16>, cx: &mut App) {
    cx.set_global(Zoom(percent.unwrap_or(100).clamp(75, 200)));
}

pub(crate) fn percent(cx: &App) -> u16 {
    cx.try_global::<Zoom>().map_or(100, |zoom| zoom.0)
}

pub(crate) fn factor(cx: &App) -> f32 {
    f32::from(percent(cx)) / 100.
}

pub(crate) fn apply(window: &mut Window, cx: &App) {
    window.set_rem_size(px(16. * f32::from(percent(cx)) / 100.));
}

pub(crate) fn set(percent: u16, window: &mut Window, cx: &mut App) {
    restore(Some(percent), cx);
    apply(window, cx);
    if let Some(attachment) = cx.try_global::<ClientAttachment>() {
        let host = attachment.host();
        let mut preferences = host.load_preferences();
        preferences.zoom_percent = Some(self::percent(cx));
        host.save_preferences(&preferences);
    }
    window.refresh();
}

pub(crate) fn step(increase: bool, window: &mut Window, cx: &mut App) {
    let current = percent(cx);
    let next = if increase {
        LEVELS.iter().copied().find(|level| *level > current)
    } else {
        LEVELS.iter().rev().copied().find(|level| *level < current)
    };
    if let Some(next) = next {
        set(next, window, cx);
    }
}

// Global actions also work on pages where no field currently holds focus.
fn in_active_window(cx: &mut App, action: fn(&mut Window, &mut App)) {
    if let Some(window) = cx.active_window() {
        // Action dispatch already borrows the window; update it after dispatch.
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| action(window, cx));
        });
    }
}

pub(crate) fn bind_keys(cx: &mut App) {
    cx.on_action(|_: &ZoomIn, cx| in_active_window(cx, |window, cx| step(true, window, cx)));
    cx.on_action(|_: &ZoomOut, cx| in_active_window(cx, |window, cx| step(false, window, cx)));
    cx.on_action(|_: &ResetZoom, cx| in_active_window(cx, |window, cx| set(100, window, cx)));

    cx.bind_keys([
        KeyBinding::new("secondary-=", ZoomIn, None),
        KeyBinding::new("secondary-+", ZoomIn, None),
        KeyBinding::new("secondary--", ZoomOut, None),
        KeyBinding::new("secondary-0", ResetZoom, None),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        overlay::OverlayHost,
        sizing::design,
        widgets::input::{Input, InputState},
    };
    use gpui::{
        AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
        ParentElement as _, Render, Styled as _, TestAppContext, div,
    };

    struct Probe(Entity<InputState>);

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child(
                div()
                    .debug_selector(|| "zoom-field".into())
                    .w(design(200.))
                    .h(design(32.))
                    .child(Input::new(&self.0)),
            )
        }
    }

    // Zoom must reach focused editors, scale their layout, and preserve the draft.
    #[gpui::test]
    fn shortcuts_scale_focused_input_and_reset_without_losing_draft(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(cx);
            restore(None, cx);
            bind_keys(cx);
        });
        let mut input = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let field = cx.new(|cx| InputState::new(window, cx).default_value("keep this draft"));
            field.read(cx).focus_handle(cx).focus(window, cx);
            input = Some(field.clone());
            let probe = cx.new(|_| Probe(field));
            OverlayHost::new(probe, window, cx)
        });
        let input = input.unwrap();
        cx.update(|window, _| window.activate_window());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(cx.debug_bounds("zoom-field").unwrap().size.width, px(200.));
        cx.simulate_keystrokes("secondary-=");
        cx.simulate_keystrokes("secondary-=");
        cx.update(|window, cx| {
            assert_eq!(percent(cx), 125);
            let _ = window.draw(cx);
        });
        assert_eq!(cx.debug_bounds("zoom-field").unwrap().size.width, px(250.));
        input.read_with(cx, |field, _| {
            assert_eq!(field.value().as_str(), "keep this draft")
        });
        cx.update(|window, cx| window.blur(cx));
        cx.simulate_keystrokes("secondary-0");
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(cx.debug_bounds("zoom-field").unwrap().size.width, px(200.));
    }
}
