//! Client-local theme import and JSON customization, shared by every platform.

use crate::sizing::design;
use crate::{
    overlay::{Notification, OverlayExt as _},
    settings::ThemeMode,
    settings_page::apply_theme,
    store::WorkspaceStore,
    theme::ActiveTheme as _,
    widgets::{
        button::{Button, ButtonVariants as _},
        input::{Textarea, TextareaState},
    },
};
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div,
};
use gpui_base::{h_flex, v_flex};

pub(crate) fn open(store: Entity<WorkspaceStore>, edit: bool, window: &mut Window, cx: &mut App) {
    let source = if edit {
        super::export_selected(cx)
    } else {
        String::new()
    };
    let editor = cx.new(|cx| ThemeEditor {
        input: cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(8, 16)
                .placeholder(crate::tr!("settings.theme.json_placeholder"))
                .default_value(source)
        }),
        store,
        error: None,
    });
    window.open_dialog(cx, move |dialog, _, cx| {
        let editor = editor.clone();
        dialog
            .w(design(680.))
            .bg(cx.theme().popover)
            .title(crate::tr!("settings.theme.editor_title").into_owned())
            .content(move |content, _, _| content.child(editor.clone()))
    });
}

struct ThemeEditor {
    input: Entity<TextareaState>,
    store: Entity<WorkspaceStore>,
    error: Option<String>,
}

impl ThemeEditor {
    #[cfg(feature = "native-dialogs")]
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(crate::tr!("settings.theme.open_file").into_owned().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = match paths.await {
                Ok(Ok(Some(mut paths))) => {
                    let Some(path) = paths.pop() else {
                        return;
                    };
                    let Ok(read) =
                        this.update(cx, |this, cx| this.store.read(cx).read_client_text(path))
                    else {
                        return;
                    };
                    read.await
                }
                Ok(Ok(None)) => return,
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(source) => {
                        this.input
                            .update(cx, |input, cx| input.set_value(source, window, cx));
                        this.error = None;
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.input.read(cx).value().to_string();
        match super::import(&source, cx) {
            Ok((converted, appearance)) => {
                let preferences = super::preferences(cx);
                let mut mode = self.store.read(cx).settings().theme_mode;
                self.store.update(cx, |store, cx| {
                    store.save_theme_preferences(preferences);
                    if appearance != cx.theme().mode {
                        mode = if appearance.is_dark() {
                            ThemeMode::Dark
                        } else {
                            ThemeMode::Light
                        };
                        store.set_client_theme(Some(mode));
                    }
                    cx.notify();
                });
                apply_theme(mode, window, cx);
                window.close_dialog(cx);
                let message = if converted {
                    crate::tr!("settings.theme.imported_vscode")
                } else {
                    crate::tr!("settings.theme.imported")
                };
                window.push_notification(Notification::success(message), cx);
            }
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }
}

impl Render for ThemeEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = crate::sizing::fit_viewport(
            design(300.).to_pixels(window.rem_size()),
            window.viewport_size().height - design(300.).to_pixels(window.rem_size()),
        );
        let mut body = v_flex()
            .gap_3()
            .child(
                div()
                    .text_size(design(13.))
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::tr!("settings.theme.import_help")),
            )
            .child(
                div()
                    .h(height)
                    .flex_shrink_0()
                    .overflow_hidden()
                    .rounded(crate::material::radius_input())
                    .child(
                        Textarea::new(&self.input)
                            .h(height)
                            .font_family(cx.theme().mono_font_family.clone()),
                    ),
            );
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .flex_shrink_0()
                    .text_size(design(12.))
                    .text_color(cx.theme().danger_foreground)
                    .child(crate::tr!(
                        "settings.theme.invalid",
                        error = error.lines().next().unwrap_or(error).to_owned()
                    )),
            );
        }
        #[cfg(feature = "native-dialogs")]
        if self.store.read(cx).supports_client_files() {
            body = body.child(
                Button::new("theme-open-file")
                    .label(crate::tr!("settings.theme.open_file"))
                    .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
            );
        }
        body.child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .justify_end()
                .child(
                    Button::new("theme-copy-json")
                        .label(crate::tr!("settings.theme.copy_json"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                this.input.read(cx).value().to_string(),
                            ));
                        })),
                )
                .child(
                    Button::new("theme-import-cancel")
                        .label(crate::tr!("settings.cancel"))
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                )
                .child(
                    Button::new("theme-import-apply")
                        .primary()
                        .label(crate::tr!("settings.theme.apply"))
                        .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                ),
        )
    }
}
