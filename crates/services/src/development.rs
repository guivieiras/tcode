//! Build tools, bounded output and immutable artifacts. Runtime owns admission.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use tcode_protocol::{BuildAttempt, BuildAttemptState, DevelopmentArtifact, DevelopmentTarget};

pub const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Records {
    pub attempts: [Option<BuildAttempt>; 2],
    pub artifacts: [Vec<DevelopmentArtifact>; 2],
}

pub fn validate_checkout(path: &Path) -> Result<PathBuf, String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("Source checkout: {error}"))?;
    let manifest = fs::read_to_string(path.join("Cargo.toml"))
        .map_err(|error| format!("Source checkout: {error}"))?;
    if !manifest.contains("[workspace]")
        || !path.join("crates/app/src/main.rs").is_file()
        || !path.join("crates/android/host/build.sh").is_file()
    {
        return Err("Choose the root of a tcode workspace".into());
    }
    Ok(path)
}

pub fn digest(path: &Path) -> io::Result<(String, u64)> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        hash.update(&buffer[..count]);
    }
    Ok((format!("{:x}", hash.finalize()), size))
}

pub fn save(root: &Path, records: &Records) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let bytes = serde_json::to_vec(records)?;
    let mut file = fs::File::create(root.join("records.new"))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(root.join("records.new"), root.join("records.json"))
}

/// Startup is the only pruning point, before clients can download artifacts.
pub fn recover(root: &Path, running: Option<&str>) -> io::Result<Records> {
    fs::create_dir_all(root)?;
    let mut records: Records = match fs::read(root.join("records.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Records::default(),
        Err(error) => return Err(error),
    };
    for attempt in records.attempts.iter_mut().flatten() {
        if attempt.state == BuildAttemptState::Running {
            attempt.state = BuildAttemptState::Interrupted;
            attempt.completed_at = Some(crate::store::now_secs());
            attempt.error = Some("Host stopped before this build finished".into());
        }
    }
    save(root, &records)?;
    let artifacts = root.join("artifacts");
    if artifacts.exists() {
        for entry in fs::read_dir(&artifacts)? {
            let entry = entry?;
            let id = entry.file_name();
            let id = id.to_string_lossy();
            let retained = running == Some(id.as_ref())
                || records.artifacts.iter().flatten().any(|a| a.build_id == id);
            if !retained && entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            }
        }
    }
    let staging = root.join("staging");
    if staging.exists() {
        fs::remove_dir_all(staging)?;
    }
    Ok(records)
}

pub fn publish(
    root: &Path,
    source: &Path,
    checkout: &Path,
    target: DevelopmentTarget,
) -> io::Result<DevelopmentArtifact> {
    let staging = root.join("staging");
    fs::create_dir_all(&staging)?;
    let name = match target {
        DevelopmentTarget::Desktop => "tcode",
        DevelopmentTarget::Android => "tcode.apk",
    };
    let staged = staging.join(name);
    fs::copy(source, &staged)?;
    fs::File::open(&staged)?.sync_all()?;
    let (build_id, size) = digest(&staged)?;
    let directory = root.join("artifacts").join(&build_id);
    fs::create_dir_all(&directory)?;
    let path = directory.join(name);
    if !path.exists() {
        fs::rename(staged, &path)?;
    } else {
        fs::remove_file(staged)?;
    }
    Ok(DevelopmentArtifact {
        build_id,
        size,
        path,
        checkout: checkout.to_owned(),
        completed_at: crate::store::now_secs(),
    })
}

pub fn record_success(
    records: &mut Records,
    target: DevelopmentTarget,
    artifact: DevelopmentArtifact,
) {
    let artifacts = &mut records.artifacts[target.index()];
    artifacts.retain(|old| old.build_id != artifact.build_id);
    artifacts.insert(0, artifact);
    artifacts.truncate(2);
}

pub fn append_output(attempt: &mut BuildAttempt, text: &str) {
    attempt.output.push_str(text);
    if attempt.output.len() > OUTPUT_LIMIT {
        let mut start = attempt.output.len() - OUTPUT_LIMIT;
        while !attempt.output.is_char_boundary(start) {
            start += 1;
        }
        attempt.output.drain(..start);
        attempt.output_truncated = true;
    }
}

/// Joining is required on normal host shutdown, including descendants of Cargo/Gradle.
pub struct BuildJob {
    cancel: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl BuildJob {
    pub fn spawn(work: impl FnOnce(Arc<AtomicBool>) + Send + 'static) -> io::Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let thread = thread::Builder::new()
            .name("development-build".into())
            .spawn(move || work(flag))?;
        Ok(Self {
            cancel,
            thread: Some(thread),
        })
    }
}
impl Drop for BuildJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn reader(
    mut pipe: impl Read + Send + 'static,
    stdout: bool,
    sender: mpsc::SyncSender<(bool, Vec<u8>)>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0; 8192];
        while let Ok(count) = pipe.read(&mut buffer) {
            if count == 0 || sender.send((stdout, buffer[..count].to_vec())).is_err() {
                break;
            }
        }
    })
}

pub fn build(
    checkout: &Path,
    target: DevelopmentTarget,
    cancel: &AtomicBool,
    mut output: impl FnMut(&str),
) -> Result<PathBuf, String> {
    let checkout = validate_checkout(checkout)?;
    let mut command = match target {
        DevelopmentTarget::Desktop => {
            let mut command = crate::process::command("cargo");
            command.args([
                "build",
                "--release",
                "--locked",
                "--bin",
                "tcode",
                "--message-format=json-render-diagnostics",
            ]);
            command
        }
        DevelopmentTarget::Android => {
            let mut command = crate::process::command("bash");
            command
                .arg(checkout.join("crates/android/host/build.sh"))
                .arg("--release");
            command
        }
    };
    command
        .current_dir(&checkout)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|error| format!("Could not start {} build: {error}. Check Cargo and the Android SDK/NDK/Java installation.", target.name()))?;
    let (sender, receiver) = mpsc::sync_channel(16);
    let stdout = reader(child.stdout.take().unwrap(), true, sender.clone());
    let stderr = reader(child.stderr.take().unwrap(), false, sender);
    let mut cargo_line = Vec::new();
    let mut artifact = None;
    let mut status = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            #[cfg(not(unix))]
            let _ = child.kill();
            let _ = child.wait();
            drop(receiver);
            let _ = stdout.join();
            let _ = stderr.join();
            return Err("Build interrupted by host shutdown".into());
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok((is_stdout, bytes)) if is_stdout && target == DevelopmentTarget::Desktop => {
                cargo_line.extend(bytes);
                while let Some(end) = cargo_line.iter().position(|b| *b == b'\n') {
                    let line: Vec<_> = cargo_line.drain(..=end).collect();
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) {
                        if value["reason"] == "compiler-artifact"
                            && value["target"]["name"] == "tcode"
                        {
                            artifact = value["executable"].as_str().map(PathBuf::from);
                        }
                        if let Some(rendered) = value["message"]["rendered"].as_str() {
                            output(rendered);
                        }
                    } else {
                        output(&String::from_utf8_lossy(&line));
                    }
                }
            }
            Ok((_, bytes)) => output(&String::from_utf8_lossy(&bytes)),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if status.is_none() {
            status = child.try_wait().map_err(|e| e.to_string())?;
            #[cfg(unix)]
            if status.is_some() {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
            }
        }
    }
    let status = status
        .map(Ok)
        .unwrap_or_else(|| child.wait())
        .map_err(|e| e.to_string())?;
    // Retire lingering tool descendants before joining the readers on normal completion.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = stdout.join();
    let _ = stderr.join();
    if !status.success() {
        return Err(format!("{} build failed ({status})", target.name()));
    }
    match target {
        DevelopmentTarget::Desktop => {
            artifact.ok_or_else(|| "Cargo did not report the tcode executable".into())
        }
        DevelopmentTarget::Android => {
            Ok(checkout.join("crates/android/host/app/build/outputs/apk/release/app-release.apk"))
        }
    }
}

/// Wait for a specific original process identity, without killing it on timeout.
pub fn wait_for_exit(mut alive: impl FnMut() -> bool, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    while alive() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Old desktop did not exit within restart deadline",
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Helper and host-side preparation failures share a recovery log outside thread history.
pub fn log_restart_error(root: &Path, error: &str) {
    let _ = fs::create_dir_all(root);
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("restart.log"))
    {
        let _ = writeln!(file, "{} {error}", crate::store::now_secs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_recover_interrupted_attempt_and_retain_success_and_running_artifact() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp")
            .join(format!("development-records-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("binary");
        fs::write(&source, b"successful executable").unwrap();
        let artifact = publish(&root, &source, &root, DevelopmentTarget::Desktop).unwrap();
        let mut records = Records::default();
        record_success(&mut records, DevelopmentTarget::Desktop, artifact.clone());
        records.attempts[0] = Some(BuildAttempt {
            attempt_id: "interrupted".into(),
            started_at: 1,
            completed_at: None,
            state: BuildAttemptState::Running,
            error: None,
            output: "compiling".into(),
            output_truncated: false,
        });
        save(&root, &records).unwrap();
        fs::create_dir_all(root.join("artifacts/running")).unwrap();
        fs::create_dir_all(root.join("artifacts/orphan")).unwrap();
        let recovered = recover(&root, Some("running")).unwrap();
        assert_eq!(recovered.artifacts[0], vec![artifact]);
        assert_eq!(
            recovered.attempts[0].as_ref().unwrap().state,
            BuildAttemptState::Interrupted
        );
        assert!(root.join("artifacts/running").exists());
        assert!(!root.join("artifacts/orphan").exists());
        let mut attempt = recovered.attempts[0].clone().unwrap();
        append_output(&mut attempt, &"字".repeat(OUTPUT_LIMIT));
        assert!(attempt.output_truncated);
        assert!(attempt.output.len() <= OUTPUT_LIMIT);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_waits_for_exit_and_timeout_does_not_launch() {
        let start = Instant::now();
        let mut launched = false;
        let result = wait_for_exit(|| true, Duration::from_millis(60));
        if result.is_ok() {
            launched = true;
        }
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(!launched);
        assert!(start.elapsed() >= Duration::from_millis(60));
        let mut polls = 0;
        wait_for_exit(
            || {
                polls += 1;
                polls < 3
            },
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(polls, 3);
    }
}
