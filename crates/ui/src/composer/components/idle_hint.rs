use super::super::*;

impl Composer {
    /// The provider's suggestion to start fresh once a large conversation has
    /// sat idle past its prompt cache.
    pub(in super::super) fn render_idle_hint(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Some(hint) = self.workspace_store.read(cx).composer_state().idle_hint else {
            self.idle_hint_wake = None;
            return None;
        };
        let now = crate::time::now_millis();
        if now < hint.due_at {
            if self
                .idle_hint_wake
                .as_ref()
                .is_none_or(|(due_at, _)| *due_at != hint.due_at)
            {
                let timer = cx
                    .background_executor()
                    .timer(Duration::from_millis(hint.due_at - now));
                let wake = cx.spawn(async move |this, cx| {
                    timer.await;
                    this.update(cx, |_, cx| cx.notify()).ok();
                });
                self.idle_hint_wake = Some((hint.due_at, wake));
            }
            return None;
        }
        self.idle_hint_wake = None;
        Some(
            div()
                .debug_selector(|| "composer-idle-hint".into())
                .mx_2()
                .px_3()
                .py_1p5()
                .when(self.compact, |el| el.py_1())
                .min_w_0()
                .rounded_t(px(12.))
                .border_1()
                .border_b_0()
                .border_color(cx.theme().border)
                .bg(cx.theme().muted)
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(crate::tr!(
                    "composer.idle_return_hint",
                    command = hint.command,
                    tokens = context_meter::format_tokens(Some(hint.tokens))
                ))
                .into_any_element(),
        )
    }
}
