//! Adapt touch holds to the renderers' existing mouse selection behavior.
use gpui::{App, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, PlatformInput, Point, Window};

pub(crate) fn select_at(
    position: Point<Pixels>,
    click_count: usize,
    window: &mut Window,
    cx: &mut App,
) {
    // Defer dispatch until the long-press listener has returned. The matching
    // mouse-up finishes selection without waiting for the captured touch release.
    window.defer(cx, move |window, cx| {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                click_count,
                modifiers: Default::default(),
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position,
                click_count,
                modifiers: Default::default(),
            }),
            cx,
        );
        #[cfg(target_os = "android")]
        gpui_android::request_selection_menu();
        window.refresh();
    });
}
