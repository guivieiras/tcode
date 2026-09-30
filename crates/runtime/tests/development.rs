#![cfg(target_os = "linux")]
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tcode_client::HostLink;
use tcode_protocol::*;
use tcode_runtime::{
    app::development::DevelopmentCapabilities,
    pipe::{HostServices, SpawnedHost, spawn_host},
};
use tcode_services::{
    development::{Records, publish, record_success, save},
    store::SessionStore,
};
use tcode_traverse::HostMux;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp")
            .join(format!("development-runtime-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(path.join("checkout/crates/app/src")).unwrap();
        fs::create_dir_all(path.join("checkout/crates/android/host")).unwrap();
        fs::write(
            path.join("checkout/Cargo.toml"),
            "[workspace]\nmembers=[]\n",
        )
        .unwrap();
        fs::write(
            path.join("checkout/crates/app/src/main.rs"),
            "fn main() {}\n",
        )
        .unwrap();
        Self(path)
    }
    fn script(&self, script: &str) {
        fs::write(self.0.join("checkout/crates/android/host/build.sh"), script).unwrap();
    }
    fn start(
        &self,
        prepare: impl Fn(&DevelopmentArtifact) -> Result<(), String> + Send + Sync + 'static,
    ) -> SpawnedHost {
        let (quit, _) = smol::channel::unbounded();
        spawn_host(
            SessionStore::open_at(self.0.join("data")).unwrap(),
            HostServices {
                development: Some(DevelopmentCapabilities {
                    checkout: self.0.join("checkout"),
                    host_name: "Test desktop".into(),
                    prepare_restart: Arc::new(prepare),
                    quit,
                }),
                ..Default::default()
            },
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn attach(mux: &HostMux) -> HostLink {
    let connection = mux.attach();
    let link = HostLink::new(connection.to_host, connection.from_host);
    let pump = link.clone();
    smol::spawn(async move { pump.pump().await }).detach();
    link
}
fn snapshot(
    link: &HostLink,
    predicate: impl Fn(&DevelopmentSnapshot) -> bool,
) -> DevelopmentSnapshot {
    link.subscribe(Subscription {
        topic: Topic::Development,
        after: None,
    })
    .unwrap();
    smol::block_on(smol::future::race(
        async {
            loop {
                if let ServerEvent::DevelopmentStateReplaced(state) =
                    link.events().recv().await.unwrap().event
                    && predicate(&state)
                {
                    return state;
                }
            }
        },
        async {
            smol::Timer::after(Duration::from_secs(10)).await;
            panic!("Development snapshot timed out");
        },
    ))
}
fn stop(host: &SpawnedHost, link: &HostLink) {
    link.command_blocking(Command::ShutdownAllAndFlush).unwrap();
    host.to_host.close();
    host.stopped.recv_blocking().unwrap();
}

#[test]
fn builds_are_host_owned_reject_overlap_and_preserve_success_after_failure_and_restart() {
    let fixture = Fixture::new();
    fixture.script("set -eu\nwhile [ ! -f release-build ]; do sleep 0.05; done\nmkdir -p crates/android/host/app/build/outputs/apk/release\nprintf 'apk artifact' > crates/android/host/app/build/outputs/apk/release/app-release.apk\necho finished\n");
    let host = fixture.start(|_| Ok(()));
    let mux = HostMux::new(host.to_host.clone(), host.from_host.clone());
    let first = attach(&mux);
    let state = snapshot(&first, |_| true);
    let command = Command::StartDevelopmentBuild {
        host_instance_id: state.host_instance_id.clone(),
        target: DevelopmentTarget::Android,
    };
    first.command_blocking(command.clone()).unwrap();
    assert!(first.command_blocking(command.clone()).is_err());
    assert!(
        first
            .command_blocking(Command::StartDevelopmentBuild {
                host_instance_id: state.host_instance_id.clone(),
                target: DevelopmentTarget::Desktop,
            })
            .is_err()
    );
    assert!(
        first
            .command_blocking(Command::PatchSettings {
                patch: SettingsPatch::DevelopmentCheckout(None)
            })
            .is_err()
    );
    first.close();
    fs::write(fixture.0.join("checkout/release-build"), "").unwrap();
    let second = attach(&mux);
    let completed = snapshot(&second, |s| {
        s.targets[1]
            .attempt
            .as_ref()
            .is_some_and(|a| a.state == BuildAttemptState::Succeeded)
    });
    let artifact = completed.targets[1].artifact.clone().unwrap();
    fixture.script("echo deliberate compiler failure >&2\nexit 1\n");
    second.command_blocking(command).unwrap();
    let failed = snapshot(&second, |s| {
        s.targets[1]
            .attempt
            .as_ref()
            .is_some_and(|a| a.state == BuildAttemptState::Failed)
    });
    assert_eq!(failed.targets[1].artifact.as_ref(), Some(&artifact));
    assert!(
        failed.targets[1]
            .attempt
            .as_ref()
            .unwrap()
            .output
            .contains("deliberate compiler failure")
    );
    stop(&host, &second);
    let restored = fixture.start(|_| Ok(()));
    let link = restored.link();
    let state = snapshot(&link, |_| true);
    assert_eq!(state.targets[1].artifact, Some(artifact));
    assert_eq!(
        state.targets[1].attempt.as_ref().unwrap().state,
        BuildAttemptState::Failed
    );
    assert!(
        link.command_blocking(Command::StartDevelopmentBuild {
            host_instance_id: completed.host_instance_id,
            target: DevelopmentTarget::Android
        })
        .is_err()
    );
    stop(&restored, &link);
}

#[test]
fn stale_and_replayed_restart_requests_only_prepare_once() {
    let fixture = Fixture::new();
    fixture.script("exit 0\n");
    let root = fixture.0.join("data/development");
    fs::create_dir_all(&root).unwrap();
    fs::write(fixture.0.join("desktop"), b"different executable").unwrap();
    let artifact = publish(
        &root,
        &fixture.0.join("desktop"),
        Path::new("checkout"),
        DevelopmentTarget::Desktop,
    )
    .unwrap();
    let mut records = Records::default();
    record_success(&mut records, DevelopmentTarget::Desktop, artifact.clone());
    save(&root, &records).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let host = fixture.start(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let link = host.link();
    let state = snapshot(&link, |_| true);
    let command = |instance| Command::RestartDevelopmentDesktop {
        host_instance_id: instance,
        build_id: artifact.build_id.clone(),
        allow_interrupt: true,
    };
    assert!(link.command_blocking(command("stale".into())).is_err());
    link.command_blocking(command(state.host_instance_id.clone()))
        .unwrap();
    assert!(
        link.command_blocking(command(state.host_instance_id))
            .is_err()
    );
    snapshot(&link, |s| s.restart == DevelopmentRestartState::Restarting);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    stop(&host, &link);
}

#[test]
fn shutdown_reaps_build_group_and_recovers_interruption() {
    let fixture = Fixture::new();
    fixture.script("sleep 300 &\necho $! > descendant\nwait\n");
    let host = fixture.start(|_| Ok(()));
    let link = host.link();
    let state = snapshot(&link, |_| true);
    link.command_blocking(Command::StartDevelopmentBuild {
        host_instance_id: state.host_instance_id,
        target: DevelopmentTarget::Android,
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.0.join("checkout/descendant").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = fs::read_to_string(fixture.0.join("checkout/descendant")).unwrap();
    stop(&host, &link);
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid.trim()));
    assert!(stat.is_err() || stat.unwrap().rsplit_once(") ").unwrap().1.starts_with('Z'));
    let restored = fixture.start(|_| Ok(()));
    let link = restored.link();
    let state = snapshot(&link, |_| true);
    assert_eq!(
        state.targets[1].attempt.as_ref().unwrap().state,
        BuildAttemptState::Interrupted
    );
    stop(&restored, &link);
}
