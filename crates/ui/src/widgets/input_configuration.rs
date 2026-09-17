//! Native keyboard configuration and Android text selection for the upstream editor.
use gpui::{
    App, Bounds, ClipboardItem, ElementInputHandler, Entity, InputHandler, Pixels, Point,
    TextInputAction, TextInputConfiguration, UTF16Selection, Window,
};
use gpui_base::input::{InputBaseState, InputModeKind, RopeExt as _};
use std::ops::Range;

#[cfg(any(target_os = "android", test))]
pub(super) fn select_word_at<M: InputModeKind>(
    entity: &Entity<InputBaseState<M>>,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    use gpui::{EntityInputHandler as _, Focusable as _};
    use unicode_segmentation::UnicodeSegmentation as _;

    entity.update(cx, |state, cx| {
        if state.presentation().is_disabled() {
            return false;
        }
        let Some(index) = state.character_index_for_point(position, window, cx) else {
            crate::touch_selection::select_at(position, 1, window, cx);
            return true;
        };
        let text = state.text();
        let offset = text.offset_utf16_to_offset(index);
        let range = if state.presentation().is_masked() {
            0..text.len()
        } else {
            // Unicode word boundaries keep combining marks and emoji sequences intact.
            let value = text.to_string();
            let Some((start, word)) = value
                .split_word_bound_indices()
                .find(|(start, word)| *start <= offset && offset < start + word.len())
            else {
                state.focus_handle(cx).focus(window, cx);
                state.set_selected_range(offset..offset, cx);
                return true;
            };
            start..start + word.len()
        };
        state.focus_handle(cx).focus(window, cx);
        state.set_selected_range(range, cx);
        true
    })
}

#[cfg(any(target_os = "android", test))]
pub(super) fn touch_selection<M: InputModeKind>(
    entity: Entity<InputBaseState<M>>,
) -> impl gpui::IntoElement {
    use gpui::{HitboxBehavior, Styled as _, TouchPhase};
    gpui::canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            window.on_mouse_event(move |event: &gpui::LongPressEvent, phase, window, cx| {
                if phase.bubble()
                    && event.phase == TouchPhase::Started
                    && hitbox.is_hovered(window)
                    && select_word_at(&entity, event.position, window, cx)
                {
                    window.capture_long_press(&entity);
                    window.prevent_default();
                    cx.stop_propagation();
                    #[cfg(target_os = "android")]
                    gpui_android::request_selection_menu();
                }
            });
        },
    )
    .absolute()
    .size_full()
}

// Supply the native selection/length hooks and keyboard intent missing upstream.
pub(super) struct ConfiguredInput<M: InputModeKind> {
    inner: ElementInputHandler<InputBaseState<M>>,
    entity: Entity<InputBaseState<M>>,
    multi_line: bool,
}

impl<M: InputModeKind> ConfiguredInput<M> {
    pub fn new(
        bounds: Bounds<Pixels>,
        entity: Entity<InputBaseState<M>>,
        multi_line: bool,
    ) -> Self {
        Self {
            inner: ElementInputHandler::new(bounds, entity.clone()),
            entity,
            multi_line,
        }
    }

    #[cfg(target_os = "android")]
    pub fn sync_selection_menu(&mut self, window: &mut Window, cx: &mut App) {
        let state = self.entity.read(cx);
        let selection = state.selected_range();
        let text = state.text();
        let selection = text.offset_to_offset_utf16(selection.start)
            ..text.offset_to_offset_utf16(selection.end);
        let presentation = state.presentation();
        let copy = !presentation.is_masked() && !presentation.is_disabled();
        let paste = state.is_editable();
        let bounds = self
            .bounds_for_range(selection.start..selection.start, window, cx)
            .unwrap_or_default();
        gpui_android::selection_menu(
            self.entity.entity_id().as_u64(),
            selection,
            bounds.to_device_pixels(window.scale_factor()),
            copy,
            copy && paste,
            paste,
            true,
        );
    }
}

impl<M: InputModeKind> InputHandler for ConfiguredInput<M> {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.inner
            .selected_text_range(ignore_disabled_input, window, cx)
    }
    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.inner.marked_text_range(window, cx)
    }
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.inner
            .text_for_range(range_utf16, adjusted_range, window, cx)
    }
    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner
            .replace_text_in_range(replacement_range, text, window, cx)
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.replace_and_mark_text_in_range(
            range_utf16,
            new_text,
            new_selected_range,
            window,
            cx,
        )
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.inner.unmark_text(window, cx)
    }
    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut App) {
        self.inner.paste(item, window, cx)
    }
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.inner.bounds_for_range(range_utf16, window, cx)
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.inner.character_index_for_point(point, window, cx)
    }
    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.entity.update(cx, |state, cx| {
            let text = state.text();
            let range = text.offset_utf16_to_offset(range_utf16.start)
                ..text.offset_utf16_to_offset(range_utf16.end);
            if state.selected_range() != range {
                state.set_selected_range(range, cx);
            }
        });
    }
    fn element_bounds(&mut self, window: &mut Window, cx: &mut App) -> Option<Bounds<Pixels>> {
        self.inner.element_bounds(window, cx)
    }
    fn text_length_utf16(&mut self, _window: &mut Window, cx: &mut App) -> Option<usize> {
        Some(self.entity.read(cx).text().len_utf16())
    }
    fn apple_press_and_hold_enabled(&mut self) -> bool {
        self.inner.apple_press_and_hold_enabled()
    }
    fn accepts_text_input(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.inner.accepts_text_input(window, cx)
    }
    fn text_input_editable_range(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Range<usize>> {
        self.inner.text_input_editable_range(window, cx)
    }
    fn prefers_ime_for_printable_keys(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.inner.prefers_ime_for_printable_keys(window, cx)
    }
    fn text_input_configuration(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> TextInputConfiguration {
        let mut configuration = self.inner.text_input_configuration(window, cx);
        configuration.input_action = if self.multi_line {
            TextInputAction::Enter
        } else {
            TextInputAction::Done
        };
        configuration
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, px,
    };
    use gpui_base::input::TextareaState;

    struct TouchInput(Entity<TextareaState>);

    impl Render for TouchInput {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div()
                .w(px(300.))
                .h(px(100.))
                .child(super::super::Textarea::new(&self.0))
        }
    }

    #[gpui::test]
    fn hold_selects_the_word_after_an_emoji_and_release_preserves_it(cx: &mut TestAppContext) {
        use gpui::{PlatformInput, TouchEvent, TouchId, TouchPhase};
        cx.update(crate::theme::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            TouchInput(cx.new(|cx| TextareaState::new(window, cx).default_value("😀 example here")))
        });
        let input = view.read_with(cx, |view, _| view.0.clone());
        let position = cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            ConfiguredInput::new(Bounds::default(), input.clone(), true)
                .bounds_for_range(4..5, window, cx)
                .unwrap()
                .center()
        });
        let touch = |phase, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                window.dispatch_event(
                    PlatformInput::Touch(TouchEvent {
                        id: TouchId(1),
                        phase,
                        position,
                        predicted_position: None,
                        force: None,
                    }),
                    cx,
                );
            });
        };
        touch(TouchPhase::Started, cx);
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(801));
        cx.run_until_parked();
        assert_eq!(
            input.read_with(cx, |input, _| input.selected_range()),
            5..12
        );
        touch(TouchPhase::Ended, cx);
        assert_eq!(
            input.read_with(cx, |input, _| input.selected_range()),
            5..12
        );
    }

    #[gpui::test]
    fn native_selection_replaces_the_word_after_an_emoji(cx: &mut TestAppContext) {
        cx.update(crate::theme::init);
        let (input, cx) = cx.add_window_view(|window, cx| {
            TextareaState::new(window, cx).default_value("😀 exmple here")
        });
        cx.update(|window, cx| {
            let mut handler = ConfiguredInput::new(Bounds::default(), input.clone(), true);
            assert_eq!(handler.text_length_utf16(window, cx), Some(14));
            handler.set_selected_text_range(3..9, window, cx);
            handler.replace_text_in_range(None, "example", window, cx);
            assert_eq!(input.read(cx).value(), "😀 example here");
            assert_eq!(
                handler.selected_text_range(true, window, cx).unwrap().range,
                10..10
            );
        });
    }
}
