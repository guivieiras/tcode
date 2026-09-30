//! Read-only host files. The dialog owns pending reads and video playback.
#[cfg(all(
    feature = "native-preview",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod video;
use crate::{overlay::OverlayExt as _, store::WorkspaceStore, theme::ActiveTheme as _};
use gpui::{
    Action, App, AppContext as _, Context, Entity, Focusable as _, Image, IntoElement,
    ParentElement as _, Render, Styled as _, Task, Window, div, img, px,
};
use gpui_base::input::{EditorState, Position};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
use tcode_protocol::{FilePreview, FilePreviewContent};

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = tcode_file, no_json)]
pub(crate) struct OpenFilePreview {
    pub target: String,
    pub base_dir: Option<PathBuf>,
}

enum Content {
    Loading,
    #[cfg(all(
        feature = "native-preview",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    Video(Entity<video::VideoView>),
    Text(Entity<EditorState>),
    Image(Arc<Image>),
    Error(String),
}

struct FilePreviewView {
    content: Content,
    pending_line: Option<u32>,
    _task: Option<Task<()>>,
    #[cfg(all(feature = "native-preview", target_os = "android"))]
    video_task: Option<Task<()>>,
}

pub(crate) fn open(
    store: Entity<WorkspaceStore>,
    action: &OpenFilePreview,
    window: &mut Window,
    cx: &mut App,
) {
    let request = store.update(cx, |store, cx| {
        store.preview_file(action.target.clone(), action.base_dir.clone(), cx)
    });
    let view = cx.new(|cx| {
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let _ = this.update_in(cx, |this: &mut FilePreviewView, window, cx| {
                match result {
                    Ok(preview) => this.loaded(preview, &store, window, cx),
                    Err(error) => this.content = Content::Error(error),
                }
                cx.notify();
            });
        });
        FilePreviewView {
            content: Content::Loading,
            pending_line: None,
            _task: Some(task),
            #[cfg(all(feature = "native-preview", target_os = "android"))]
            video_task: None,
        }
    });
    let title = action.target.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = view.clone();
        dialog
            .w(px(1200.))
            .rounded(crate::material::radius_overlay())
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_xl()
            .title(title.clone())
            .content(move |content, _, _| content.child(view.clone()))
    });
}

impl FilePreviewView {
    fn loaded(
        &mut self,
        preview: FilePreview,
        _store: &Entity<WorkspaceStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.content = match preview.content {
            FilePreviewContent::Text { text, line } => {
                let editor = cx.new(|cx| {
                    let mut editor = EditorState::new(window, cx).line_number(true);
                    editor.set_value(text, window, cx);
                    editor.set_readonly(true, cx);
                    editor
                });
                self.pending_line = line;
                Content::Text(editor)
            }
            FilePreviewContent::Image { bytes } => match image::guess_format(&bytes)
                .ok()
                .and_then(|format| gpui::ImageFormat::from_mime_type(format.to_mime_type()))
            {
                Some(format) => Content::Image(Arc::new(Image::from_bytes(format, bytes))),
                None => Content::Error(crate::tr!("file_preview.unsupported").into_owned()),
            },
            FilePreviewContent::Video { size, mime } => {
                let audio_only = mime.starts_with("audio/");
                #[cfg(all(feature = "native-preview", target_os = "android"))]
                {
                    match _store
                        .read(cx)
                        .video_stream(preview.path.clone(), size, mime)
                    {
                        Ok(stream) => {
                            let title = preview
                                .path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned();
                            let error = crate::tr!("file_preview.video_error").into_owned();
                            let close = crate::tr!("file_preview.close").into_owned();
                            let controls = serde_json::json!({
                                "size": size,
                                "audio": if audio_only { crate::tr!("file_preview.audio").into_owned() } else { String::new() },
                                "play": crate::tr!("file_preview.play"),
                                "pause": crate::tr!("file_preview.pause"),
                                "restart": crate::tr!("file_preview.restart"),
                                "mute": crate::tr!("file_preview.mute"),
                                "unmute": crate::tr!("file_preview.unmute"),
                                "seek": crate::tr!("file_preview.seek"),
                            })
                            .to_string();
                            self.video_task = Some(cx.spawn_in(window, async move |this, cx| {
                                let result = gpui_android::video::play(
                                    stream.url().into(),
                                    title,
                                    error,
                                    close,
                                    controls,
                                )
                                .await;
                                drop(stream);
                                let _ = this.update_in(cx, |this, window, cx| match result {
                                    Ok(()) => window.close_dialog(cx),
                                    Err(error) => {
                                        this.content = Content::Error(error);
                                        cx.notify();
                                    }
                                });
                            }));
                            Content::Loading
                        }
                        Err(error) => Content::Error(error.to_string()),
                    }
                }
                #[cfg(all(
                    feature = "native-preview",
                    any(target_os = "linux", target_os = "macos", target_os = "windows")
                ))]
                {
                    match _store.read(cx).video_stream(preview.path, size, mime) {
                        Ok(stream) => Content::Video(
                            cx.new(|cx| video::VideoView::new(stream, audio_only, window, cx)),
                        ),
                        Err(error) => Content::Error(error.to_string()),
                    }
                }
                #[cfg(not(all(
                    feature = "native-preview",
                    any(
                        target_os = "android",
                        target_os = "linux",
                        target_os = "macos",
                        target_os = "windows"
                    )
                )))]
                {
                    let _ = (size, mime, audio_only);
                    Content::Error(crate::tr!("file_preview.video_unavailable").into_owned())
                }
            }
        };
    }
}

impl Render for FilePreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(line) = self.pending_line.take()
            && let Content::Text(editor) = &self.content
        {
            let editor = editor.clone();
            // Cursor-based scrolling needs the editor's first measured viewport.
            window.on_next_frame(move |window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.set_cursor_position(
                        Position::new(line.saturating_sub(1), 0),
                        window,
                        cx,
                    );
                });
            });
        }
        let height = (window.viewport_size().height * 0.75)
            .min(window.viewport_size().height - px(140.))
            .max(px(80.));
        let content = match &self.content {
            #[cfg(all(
                feature = "native-preview",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            Content::Video(view) => view.clone().into_any_element(),
            Content::Loading => div()
                .p_4()
                .child(crate::tr!("file_preview.loading").into_owned())
                .into_any_element(),
            Content::Error(error) => div()
                .p_4()
                .text_color(cx.theme().danger)
                .child(crate::tr!("file_preview.error", error = error.clone()).into_owned())
                .into_any_element(),
            // gpui-base's editor owns its touch pans through GPUI's scroll events.
            Content::Text(editor) => div()
                .size_full()
                .relative()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(13.))
                .line_height(px(21.))
                .child(editor.clone())
                .child({
                    let editor = editor.clone();
                    gpui::canvas(
                        |_, _, _| (),
                        move |bounds, _, window, cx| {
                            let focus = editor.focus_handle(cx);
                            let bounds = editor.read(cx).text_bounds().unwrap_or(bounds);
                            window.handle_input(
                                &focus,
                                crate::widgets::input::input_configuration::ConfiguredInput::new(
                                    bounds,
                                    editor.clone(),
                                    true,
                                ),
                                cx,
                            );
                        },
                    )
                    .absolute()
                    .size_full()
                })
                .into_any_element(),
            Content::Image(image) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(img(image.clone()).max_w_full().max_h(height))
                .into_any_element(),
        };
        div()
            .w_full()
            .h(height)
            .min_w_0()
            .overflow_hidden()
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn linked_line_is_revealed_after_the_preview_is_laid_out(cx: &mut TestAppContext) {
        cx.update(crate::theme::init);
        let (to_host, _) = async_channel::unbounded();
        let (_, from_host) = async_channel::unbounded();
        let store =
            cx.new(|cx| WorkspaceStore::new(tcode_client::HostLink::new(to_host, from_host), cx));
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = FilePreviewView {
                content: Content::Loading,
                pending_line: None,
                _task: None,
                #[cfg(all(feature = "native-preview", target_os = "android"))]
                video_task: None,
            };
            view.loaded(
                FilePreview {
                    path: PathBuf::from("/host-only/source.rs"),
                    content: FilePreviewContent::Text {
                        text: (1..=160).map(|n| format!("source line {n}\n")).collect(),
                        line: Some(90),
                    },
                },
                &store,
                window,
                cx,
            );
            view
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            let Content::Text(editor) = &view.content else {
                panic!("text preview");
            };
            assert_eq!(editor.read(cx).cursor_position().line, 89);
            assert!(
                editor.read(cx).scroll_offset().y < px(-500.),
                "line 90 must be visible, not just selected offscreen"
            );
        });
    }
}
