//! Desktop video surface. Dropping the view closes the decoder and its stream.
#[path = "player.rs"]
mod player;
use crate::{theme::ActiveTheme as _, widgets::Button};
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, RenderImage,
    Styled as _, StyledImage as _, Subscription, Task, Window, div, img, px,
};
use gpui_base::slider::{
    Slider, SliderEvent, SliderIndicator, SliderState, SliderThumb, SliderTrack, SliderValue,
};
use player::{Command, Error, Player};
use std::sync::Arc;
use tcode_traverse::file_stream::FileStream;

pub(super) struct VideoView {
    player: Player,
    frame: Option<Arc<RenderImage>>,
    position: f64,
    duration: f64,
    paused: bool,
    muted: bool,
    ended: bool,
    error: Option<String>,
    seek: Entity<SliderState>,
    seeking: bool,
    _seek_subscription: Subscription,
    _updates: Task<()>,
}

impl VideoView {
    pub fn new(stream: FileStream, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (player, updates) = Player::new(stream);
        let seek = cx.new(|_| SliderState::new().min(0.).max(1000.).step(1.));
        let subscription = cx.subscribe_in(&seek, window, |this, _, event, _, cx| {
            match event {
                SliderEvent::Change(_) => this.seeking = true,
                SliderEvent::Release(SliderValue::Single(value)) => {
                    this.player
                        .send(Command::Seek(this.duration * f64::from(*value) / 1000.));
                    this.seeking = false;
                }
                _ => {}
            }
            cx.notify();
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            while updates.recv().await.is_ok() {
                if this
                    .update_in(cx, |this: &mut Self, window, cx| this.refresh(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            player,
            frame: None,
            position: 0.,
            duration: 0.,
            paused: false,
            muted: false,
            ended: false,
            error: None,
            seek,
            seeking: false,
            _seek_subscription: subscription,
            _updates: task,
        }
    }
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut state = self.player.snapshot.lock().unwrap();
        self.position = state.position;
        self.duration = state.duration;
        self.paused = state.paused;
        self.muted = state.muted;
        self.ended = state.ended;
        if let Some(frame) = state.frame.take() {
            let pixels =
                image::RgbaImage::from_raw(frame.width, frame.height, frame.bytes).unwrap();
            self.frame = Some(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])));
        }
        if let Some(error) = &state.error {
            self.error = Some(match error {
                Error::Unavailable => crate::tr!("file_preview.desktop_runtime").into_owned(),
                Error::Playback(detail) => {
                    crate::tr!("file_preview.error", error = detail.clone()).into_owned()
                }
            });
        }
        if !self.seeking && self.duration > 0. {
            self.seek.update(cx, |slider, cx| {
                slider.set_value((self.position / self.duration * 1000.) as f32, window, cx)
            });
        }
        cx.notify();
    }
    fn seek_bar(&self, disabled: bool, cx: &Context<Self>) -> impl IntoElement {
        let fraction = self.seek.read(cx).percentage().end;
        Slider::new(&self.seek)
            .w_full()
            .h(px(28.))
            .disabled(disabled)
            .child(
                SliderTrack::new(&self.seek)
                    .relative()
                    .size_full()
                    .disabled(disabled)
                    .child(
                        div()
                            .absolute()
                            .top(px(12.))
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(cx.theme().border),
                    )
                    .child(
                        SliderIndicator::new(&self.seek)
                            .absolute()
                            .top(px(12.))
                            .w_full()
                            .h(px(4.))
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(fraction))
                                    .rounded_full()
                                    .bg(cx.theme().link),
                            ),
                    )
                    .child(
                        SliderThumb::new(&self.seek)
                            .absolute()
                            .top(px(6.))
                            .left(gpui::relative(fraction))
                            .ml(px(-8.))
                            .size(px(16.))
                            .rounded_full()
                            .bg(cx.theme().link)
                            .disabled(disabled),
                    ),
            )
    }

    fn toggle_play(&mut self) {
        if self.ended {
            self.player.send(Command::Restart);
        } else {
            self.player.send(Command::Pause(!self.paused));
        }
    }
}

impl Render for VideoView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.frame.is_some() && self.error.is_none();
        let picture = if let Some(error) = &self.error {
            div()
                .p_4()
                .text_color(cx.theme().danger)
                .child(error.clone())
                .into_any_element()
        } else if let Some(frame) = &self.frame {
            img(frame.clone())
                .absolute()
                .size_full()
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element()
        } else {
            div()
                .p_4()
                .child(crate::tr!("file_preview.loading").into_owned())
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .gap_2()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(picture),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .child(self.seek_bar(!ready || self.duration <= 0., cx)),
                    )
                    .child(div().text_size(px(12.)).child(format!(
                        "{} / {}",
                        timestamp(self.position),
                        timestamp(self.duration)
                    ))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("video-play")
                            .label(
                                crate::tr!(if self.paused || self.ended {
                                    "file_preview.play"
                                } else {
                                    "file_preview.pause"
                                })
                                .into_owned(),
                            )
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, _, _| this.toggle_play())),
                    )
                    .child(
                        Button::new("video-restart")
                            .label(crate::tr!("file_preview.restart").into_owned())
                            .disabled(!ready)
                            .on_click(
                                cx.listener(|this, _, _, _| this.player.send(Command::Restart)),
                            ),
                    )
                    .child(
                        Button::new("video-mute")
                            .label(
                                crate::tr!(if self.muted {
                                    "file_preview.unmute"
                                } else {
                                    "file_preview.mute"
                                })
                                .into_owned(),
                            )
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, _, _| {
                                this.player.send(Command::Mute(!this.muted))
                            })),
                    ),
            )
    }
}

fn timestamp(seconds: f64) -> String {
    let seconds = seconds.max(0.) as u64;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}
