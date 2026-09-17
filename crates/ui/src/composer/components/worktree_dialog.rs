//! Configure a draft's named worktree without creating it before the first send.
use super::super::*;
use crate::overlay::DialogActions;
use crate::sizing::design;
use crate::store::{TopicKind, observe_store_topics};
use gpui::ScrollHandle;
use gpui_base::{Scrollbar, ScrollbarMode};

pub(super) fn open(store: Entity<WorkspaceStore>, window: &mut Window, cx: &mut App) {
    store.update(cx, |store, _| store.load_branches());
    let form = cx.new(|cx| WorktreeDialog::new(store, window, cx));
    let focus = form.read(cx).name.clone();
    window.open_dialog(cx, move |dialog, window, _| {
        let content = form.clone();
        let confirm = form.clone();
        let keyboard_confirm = form.clone();
        dialog
            .title(crate::tr!("composer.new_worktree"))
            .width(crate::sizing::fit_viewport(
                design(440.).to_pixels(window.rem_size()),
                window.viewport_size().width - design(32.).to_pixels(window.rem_size()),
            ))
            .content(move |content_el, _, _| content_el.child(content.clone()))
            .on_ok(move |_, window, cx| {
                keyboard_confirm.update(cx, |form, cx| form.confirm(window, cx))
            })
            .footer(
                DialogActions::new()
                    .child(
                        Button::new("worktree-cancel")
                            .label(crate::tr!("settings.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("worktree-confirm")
                            .primary()
                            .label(crate::tr!("composer.worktree_use"))
                            .on_click(move |_, window, cx| {
                                if confirm.update(cx, |form, cx| form.confirm(window, cx)) {
                                    window.close_dialog(cx);
                                }
                            }),
                    ),
            )
    });
    focus.update(cx, |input, cx| input.focus(window, cx));
}

struct WorktreeDialog {
    store: Entity<WorkspaceStore>,
    name: Entity<InputState>,
    base: String,
    error: Option<String>,
    branch_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl WorktreeDialog {
    fn new(store: Entity<WorkspaceStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = store
            .read(cx)
            .composer_state()
            .checkout
            .map(|checkout| checkout.workspace);
        let (name, base) = match workspace {
            Some(WorkspaceMode::NewWorktree { name, base }) => (name, base),
            _ => (String::new(), "main".to_string()),
        };
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("feature/search");
            input.set_value(name, window, cx);
            input
        });
        let store_subscription = observe_store_topics(&store, &[TopicKind::SessionStatus], cx);
        let input_subscription = cx.subscribe_in(&name, window, |form, _, event, _, cx| {
            if let InputEvent::Change = event {
                form.error = None;
                cx.notify();
            }
        });
        Self {
            store,
            name,
            base,
            error: None,
            branch_scroll: ScrollHandle::new(),
            _subscriptions: vec![store_subscription, input_subscription],
        }
    }

    fn confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        let name = self.name.read(cx).value().trim().to_string();
        let branches = self
            .store
            .read(cx)
            .composer_state()
            .checkout
            .map(|checkout| checkout.branches)
            .unwrap_or_default();
        let error = if name.is_empty() {
            Some(crate::tr!("composer.worktree_name_required").into_owned())
        } else if branches.contains(&name) {
            Some(crate::tr!("composer.worktree_name_exists").into_owned())
        } else if !branches.contains(&self.base) {
            Some(crate::tr!("composer.worktree_base_required").into_owned())
        } else {
            None
        };
        if error.is_some() {
            self.error = error;
            cx.notify();
            return false;
        }
        let base = self.base.clone();
        self.store.update(cx, |store, _| {
            store.set_draft_workspace(WorkspaceMode::NewWorktree { name, base })
        });
        true
    }
}

impl Render for WorktreeDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let branches = self
            .store
            .read(cx)
            .composer_state()
            .checkout
            .map(|checkout| checkout.branches)
            .unwrap_or_default();
        let selected = self.base.clone();
        let form = cx.entity();
        let scroll = self.branch_scroll.clone();
        let picker = crate::material::overlay_popover("worktree-base-popover")
            .anchor(Anchor::TopLeft)
            .trigger(
                Button::new("worktree-base").outline().w_full().child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_2()
                        .child(Icon::empty().path("icons/git-branch.svg").small())
                        .child(
                            div()
                                .flex_1()
                                .text_left()
                                .truncate()
                                .child(self.base.clone()),
                        )
                        .child(Icon::new(IconName::ChevronDown).xsmall()),
                ),
            )
            .content(move |_, window, cx| {
                use crate::touch_scroll::TouchScrollExt as _;
                let popover = cx.entity();
                let mut list = v_flex()
                    .id("worktree-base-list")
                    .w(crate::sizing::fit_viewport(
                        design(360.).to_pixels(window.rem_size()),
                        window.viewport_size().width - design(64.).to_pixels(window.rem_size()),
                    ))
                    .max_h(design(240.))
                    .touch_overflow_y_scroll()
                    .track_scroll(&scroll)
                    .p_1()
                    .pr(design(16.))
                    .gap_0p5();
                if branches.is_empty() {
                    list = list.child(div().p_2().child(crate::tr!("composer.loading")));
                }
                for (index, branch) in branches.iter().enumerate() {
                    let name = branch.clone();
                    let form = form.clone();
                    let popover = popover.clone();
                    list = list.child(
                        Button::new(("worktree-base-option", index))
                            .ghost()
                            .compact()
                            .w_full()
                            .flex_none()
                            .selected(*branch == selected)
                            .aria_label(branch.clone())
                            .child(div().w_full().text_left().truncate().child(branch.clone()))
                            .on_click(move |_, window, cx| {
                                form.update(cx, |form, cx| {
                                    form.base = name.clone();
                                    form.error = None;
                                    cx.notify();
                                });
                                popover.update(cx, |state, cx| state.dismiss(window, cx));
                            }),
                    );
                }
                div().relative().child(list).child(
                    div().absolute().inset_0().child(
                        Scrollbar::vertical(&scroll)
                            .id("worktree-base-scrollbar")
                            .mode(ScrollbarMode::Always),
                    ),
                )
            });
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_size(design(13.))
                            .child(crate::tr!("composer.worktree_name")),
                    )
                    .child(Input::new(&self.name).aria_label(crate::tr!("composer.worktree_name"))),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_size(design(13.))
                            .child(crate::tr!("composer.worktree_base")),
                    )
                    .child(picker),
            )
            .child(
                div()
                    .text_size(design(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::tr!("composer.worktree_on_send")),
            )
            .when_some(self.error.clone(), |form, error| {
                form.child(
                    div()
                        .text_size(design(12.))
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
    }
}
