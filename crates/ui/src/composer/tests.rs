use super::*;
use gpui::{EntityInputHandler as _, TestAppContext, VisualTestContext, size};
use tcode_runtime::pipe::{HostServices, spawn_host};
use tcode_services::store::SessionStore;

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

fn mount(
    cx: &mut TestAppContext,
    compact: bool,
) -> (Entity<Composer>, PathBuf, &mut VisualTestContext) {
    cx.update(crate::theme::init);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tmp")
        .join(format!(
            "skill-picker-test-{}-{}-{compact}",
            std::process::id(),
            tcode_services::store::now_millis()
        ));
    let store = SessionStore::open_at(root.clone()).expect("test store");
    let commands = (0..20)
        .map(|index| agent::ProviderCommand {
            name: format!("skill-{index:02}"),
            description: Some(format!("Skill {index}")),
            kind: ProviderCommandKind::Skill,
        })
        .collect::<Vec<_>>();
    store
        .save_commands(ProviderKind::ClaudeCode, None, &commands)
        .unwrap();
    let host = spawn_host(store, HostServices::default()).expect("test host");
    let cwd = root.clone();
    let session_id =
        smol::block_on(host.update_state_for_test(move |state, cx| {
            state.start_draft("picker-test".into(), cwd, cx)
        }))
        .unwrap();
    let store = cx.new(|cx| WorkspaceStore::new(host.link(), cx));
    store.update(cx, |store, cx| {
        store.set_session_replica_for_test(session_id, Default::default(), cx);
    });
    let (composer, cx) =
        cx.add_window_view(|window, cx| Composer::new_with_layout(store, compact, window, cx));
    cx.simulate_resize(size(px(if compact { 393. } else { 1024. }), px(600.)));
    draw(cx);
    cx.update(|window, cx| {
        composer
            .read(cx)
            .input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    });
    draw(cx);
    (composer, root, cx)
}

#[gpui::test]
fn skill_picker_arrows_select_without_moving_the_caret(cx: &mut TestAppContext) {
    let (composer, root, cx) = mount(cx, false);
    cx.simulate_input("use $");
    draw(cx);
    cx.simulate_keystrokes("down down up");
    draw(cx);
    composer.read_with(cx, |composer, cx| {
        assert_eq!(composer.menu_highlight, 1);
        assert_eq!(composer.input.read(cx).cursor(), 5);
    });
    cx.simulate_keystrokes(&["down"; 25].join(" "));
    draw(cx);
    let menu = cx.debug_bounds("composer-trigger-menu").unwrap();
    let last = cx.debug_bounds("composer-trigger-row-19").unwrap();
    assert!(
        menu.contains(&last.center()),
        "arrows scroll to the last skill"
    );
    composer.read_with(cx, |composer, _| assert_eq!(composer.menu_highlight, 19));
    cx.simulate_keystrokes(&["up"; 25].join(" "));
    draw(cx);
    composer.read_with(cx, |composer, _| assert_eq!(composer.menu_highlight, 0));
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("enter");
    draw(cx);
    composer.read_with(cx, |composer, cx| {
        assert_eq!(composer.input.read(cx).value().as_ref(), "use $skill-01 ");
    });
    cx.simulate_input("$");
    draw(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(!composer.read_with(cx, |composer, _| composer.menu_visible()));
    cx.simulate_keystrokes("up");
    composer.read_with(cx, |composer, cx| {
        assert_eq!(composer.input.read(cx).cursor(), 0)
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui::test]
fn compact_skill_picker_opens_and_accepts_a_tapped_skill(cx: &mut TestAppContext) {
    let (composer, root, cx) = mount(cx, true);
    cx.simulate_input("use $");
    // Android IMEs can leave the query composing while the user taps a result.
    cx.update(|window, cx| {
        composer.read(cx).input.clone().update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "sk", Some(2..2), window, cx);
        });
    });
    draw(cx);
    let menu = cx
        .debug_bounds("composer-trigger-menu")
        .expect("visible compact picker");
    let row = cx
        .debug_bounds("composer-trigger-row-1")
        .expect("second skill");
    assert!(menu.contains(&row.center()));
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    draw(cx);
    composer.read_with(cx, |composer, cx| {
        assert_eq!(composer.input.read(cx).value().as_ref(), "use $skill-01 ");
        assert!(!composer.menu_visible());
    });
    let _ = std::fs::remove_dir_all(root);
}
