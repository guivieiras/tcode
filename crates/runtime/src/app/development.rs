//! Host-owned admission and build lifecycle; desktop restart is injected by composition.
use super::*;
use tcode_protocol::{
    BuildAttempt, BuildAttemptState, DevelopmentArtifact, DevelopmentRestartState,
    DevelopmentSnapshot, DevelopmentTarget, DevelopmentTargetState,
};
use tcode_services::development::{self as service, BuildJob, Records};

pub type PrepareRestart = Arc<dyn Fn(&DevelopmentArtifact) -> Result<(), String> + Send + Sync>;
pub struct DevelopmentCapabilities {
    pub checkout: PathBuf,
    pub host_name: String,
    pub prepare_restart: PrepareRestart,
    pub quit: smol::channel::Sender<()>,
}

#[derive(Default)]
pub(super) struct Development {
    capabilities: Option<DevelopmentCapabilities>,
    pub(super) snapshot: DevelopmentSnapshot,
    records: Records,
    root: PathBuf,
    job: Option<BuildJob>,
}

fn error(message: impl Into<String>) -> ProtocolError {
    ProtocolError {
        code: "development".into(),
        message: message.into(),
    }
}

impl AppState {
    pub(crate) fn attach_development(&mut self, capabilities: DevelopmentCapabilities) {
        let root = self.store.root().join("development");
        let running = service::digest(Path::new("/proc/self/exe")).map(|(id, _)| id);
        let records = service::recover(&root, running.as_ref().ok().map(String::as_str));
        let failure = running
            .as_ref()
            .err()
            .or(records.as_ref().err())
            .map(ToString::to_string);
        let checkout = self
            .settings
            .development_checkout
            .clone()
            .or_else(|| service::validate_checkout(&capabilities.checkout).ok());
        self.development = Development {
            snapshot: DevelopmentSnapshot {
                host_instance_id: uuid::Uuid::new_v4().to_string(),
                host_name: capabilities.host_name.clone(),
                available: failure.is_none(),
                unavailable_reason: failure,
                checkout,
                running_build_id: running.ok(),
                ..Default::default()
            },
            root,
            records: records.unwrap_or_default(),
            capabilities: Some(capabilities),
            job: None,
        };
    }

    pub(crate) fn development_supported(&self) -> bool {
        self.development.capabilities.is_some()
    }

    pub(crate) fn development_snapshot(&self) -> DevelopmentSnapshot {
        let mut state = self.development.snapshot.clone();
        if let Some(name) = &self.settings.remote_host_name {
            state.host_name = name.clone();
        }
        for (index, target) in state.targets.iter_mut().enumerate() {
            *target = DevelopmentTargetState {
                attempt: self.development.records.attempts[index].clone(),
                artifact: self.development.records.artifacts[index].first().cloned(),
            };
        }
        state.active_work = self.residents.ids().any(|id| {
            self.resident(id)
                .is_some_and(|r| r.has_work() || !r.terminal_workspace.terminals.is_empty())
        }) || self
            .terminal_workspaces
            .values()
            .any(|workspace| !workspace.terminals.is_empty());
        state
    }

    fn development_idle(&self) -> Result<(), ProtocolError> {
        if !self.development.snapshot.available {
            return Err(error("Development is available on Linux headed hosts only"));
        }
        if self.development.job.is_some() {
            return Err(error("A development build is already running"));
        }
        if matches!(
            self.development.snapshot.restart,
            DevelopmentRestartState::Preparing | DevelopmentRestartState::Restarting
        ) {
            return Err(error("Desktop restart is already in progress"));
        }
        Ok(())
    }

    fn development_instance(&self, instance: &str) -> Result<(), ProtocolError> {
        if instance != self.development.snapshot.host_instance_id {
            return Err(error(
                "This request belongs to an earlier host process. Reconnect and try again.",
            ));
        }
        self.development_idle()
    }

    pub(crate) fn set_development_checkout(
        &mut self,
        checkout: Option<PathBuf>,
        cx: &mut HostCx,
    ) -> Result<(), ProtocolError> {
        self.development_idle()?;
        let checkout = checkout
            .as_deref()
            .map(service::validate_checkout)
            .transpose()
            .map_err(error)?;
        let mut settings = self.settings.clone();
        settings.development_checkout = checkout.clone();
        self.development.snapshot.checkout = checkout;
        self.update_settings(settings, cx);
        Ok(())
    }

    pub(crate) fn start_development_build(
        &mut self,
        instance: &str,
        target: DevelopmentTarget,
        cx: &mut HostCx,
    ) -> Result<(), ProtocolError> {
        self.development_instance(instance)?;
        let checkout = self
            .development
            .snapshot
            .checkout
            .as_deref()
            .ok_or_else(|| error("Choose a Source checkout first"))?;
        let checkout = service::validate_checkout(checkout).map_err(error)?;
        let attempt = BuildAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            started_at: now_secs(),
            completed_at: None,
            state: BuildAttemptState::Running,
            error: None,
            output: String::new(),
            output_truncated: false,
        };
        let mut records = self.development.records.clone();
        records.attempts[target.index()] = Some(attempt.clone());
        service::save(&self.development.root, &records).map_err(|e| error(e.to_string()))?;
        self.development.records = records;
        let root = self.development.root.clone();
        let host = cx.clone();
        let job = BuildJob::spawn(move |cancel| {
            let mut attempt = attempt;
            let mut last_publish = Instant::now();
            let result = service::build(&checkout, target, &cancel, |text| {
                service::append_output(&mut attempt, text);
                if last_publish.elapsed() >= Duration::from_secs(1) {
                    let progress = attempt.clone();
                    host.enqueue(move |state, _| {
                        if state.development.job.is_some() {
                            state.development.records.attempts[target.index()] = Some(progress);
                        }
                    });
                    last_publish = Instant::now();
                }
            })
            .and_then(|source| {
                service::publish(&root, &source, &checkout, target).map_err(|e| e.to_string())
            });
            attempt.completed_at = Some(now_secs());
            attempt.state = if result.is_ok() {
                BuildAttemptState::Succeeded
            } else {
                BuildAttemptState::Failed
            };
            attempt.error = result.as_ref().err().cloned();
            host.enqueue(move |state, _| {
                let development = &mut state.development;
                if development.job.is_none() {
                    return;
                }
                let mut records = development.records.clone();
                records.attempts[target.index()] = Some(attempt);
                if let Ok(artifact) = result {
                    service::record_success(&mut records, target, artifact);
                }
                if let Err(error) = service::save(&development.root, &records) {
                    let attempt = records.attempts[target.index()].as_mut().unwrap();
                    attempt.state = BuildAttemptState::Failed;
                    attempt.error = Some(format!("Could not publish build record: {error}"));
                    records.artifacts = development.records.artifacts.clone();
                }
                development.records = records;
                development.job = None;
            });
        });
        let job = match job {
            Ok(job) => job,
            Err(failure) => {
                let attempt = self.development.records.attempts[target.index()]
                    .as_mut()
                    .unwrap();
                attempt.state = BuildAttemptState::Failed;
                attempt.completed_at = Some(now_secs());
                attempt.error = Some(failure.to_string());
                let _ = service::save(&self.development.root, &self.development.records);
                return Err(error(failure.to_string()));
            }
        };
        self.development.job = Some(job);
        Ok(())
    }

    pub(crate) fn restart_development_desktop(
        &mut self,
        instance: &str,
        build_id: &str,
        allow_interrupt: bool,
        cx: &mut HostCx,
    ) -> Result<(), ProtocolError> {
        self.development_instance(instance)?;
        let snapshot = self.development_snapshot();
        if snapshot.active_work && !allow_interrupt {
            return Err(ProtocolError {
                code: "development_confirmation_required".into(),
                message: "Restart will stop provider work and terminals".into(),
            });
        }
        if snapshot.running_build_id.as_deref() == Some(build_id) {
            return Err(error("Already running this build"));
        }
        let artifact = self.development.records.artifacts[0]
            .first()
            .filter(|a| a.build_id == build_id)
            .cloned()
            .ok_or_else(|| {
                error("The requested successful desktop build is no longer available")
            })?;
        let capabilities = self.development.capabilities.as_ref().unwrap();
        let prepare = capabilities.prepare_restart.clone();
        let quit = capabilities.quit.clone();
        self.development.snapshot.restart = DevelopmentRestartState::Preparing;
        let host = cx.clone();
        let verified = cx.unblock(move || {
            let (id, size) = service::digest(&artifact.path).map_err(|e| e.to_string())?;
            if id != artifact.build_id || size != artifact.size {
                return Err("Desktop artifact failed verification".into());
            }
            prepare(&artifact)
        });
        cx.spawn_detached(async move {
            let result = verified.await;
            host.enqueue(move |state, _| {
                state.development.snapshot.restart = match result {
                    Ok(()) => {
                        let _ = quit.try_send(());
                        DevelopmentRestartState::Restarting
                    }
                    Err(error) => {
                        service::log_restart_error(&state.development.root, &error);
                        DevelopmentRestartState::Failed(error)
                    }
                };
            });
        });
        Ok(())
    }

    pub(super) fn stop_development(&mut self) {
        if self.development.job.take().is_some() {
            for attempt in self.development.records.attempts.iter_mut().flatten() {
                if attempt.state == BuildAttemptState::Running {
                    attempt.state = BuildAttemptState::Interrupted;
                    attempt.completed_at = Some(now_secs());
                    attempt.error = Some("Host shut down during the build".into());
                }
            }
            if let Err(error) = service::save(&self.development.root, &self.development.records) {
                log::error!("Could not save interrupted build: {error}");
            }
        }
    }
}
