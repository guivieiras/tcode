//! Host-owned build state. A build includes saved, uncommitted source changes.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevelopmentTarget {
    Desktop,
    Android,
}

impl DevelopmentTarget {
    pub fn index(self) -> usize {
        match self {
            Self::Desktop => 0,
            Self::Android => 1,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Android => "android",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildAttemptState {
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildAttempt {
    pub attempt_id: String,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub state: BuildAttemptState,
    pub error: Option<String>,
    pub output: String,
    pub output_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevelopmentArtifact {
    pub build_id: String,
    pub path: PathBuf,
    pub size: u64,
    pub checkout: PathBuf,
    pub completed_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevelopmentTargetState {
    pub attempt: Option<BuildAttempt>,
    pub artifact: Option<DevelopmentArtifact>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevelopmentRestartState {
    #[default]
    Idle,
    Preparing,
    Restarting,
    Failed(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevelopmentSnapshot {
    pub host_instance_id: String,
    pub host_name: String,
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub checkout: Option<PathBuf>,
    pub running_build_id: Option<String>,
    pub targets: [DevelopmentTargetState; 2],
    pub restart: DevelopmentRestartState,
    pub active_work: bool,
}

impl DevelopmentSnapshot {
    pub fn building(&self) -> bool {
        self.targets.iter().any(|target| {
            target
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.state == BuildAttemptState::Running)
        })
    }
}
