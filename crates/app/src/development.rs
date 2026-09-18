//! Linux desktop restart composition. The helper runs before GPUI or the store opens.
use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tcode_runtime::app::development::DevelopmentCapabilities;
use tcode_services::{
    development::{digest, wait_for_exit},
    process::command,
};

fn process_identity(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return None;
    }
    fields.get(19).map(|value| value.to_string())
}

pub fn capabilities(
    data_dir: PathBuf,
    host_name: String,
    quit: smol::channel::Sender<()>,
) -> DevelopmentCapabilities {
    DevelopmentCapabilities {
        checkout: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
        host_name,
        quit,
        prepare_restart: Arc::new(move |artifact| {
            let result = prepare(&data_dir, &artifact.path);
            if result.is_ok() {
                let marker = tcode_services::relaunch::RelaunchMarker {
                    reopen_settings: "development".into(),
                    active_session: None,
                };
                if let Err(error) = tcode_services::relaunch::write(&data_dir, &marker) {
                    log_error(&data_dir, &error.to_string());
                }
            }
            if let Err(error) = &result {
                log_error(&data_dir, error);
            }
            result
        }),
    }
}

fn log_error(data_dir: &Path, error: &str) {
    tcode_services::development::log_restart_error(&data_dir.join("development"), error);
}

fn prepare(data_dir: &Path, artifact: &Path) -> Result<(), String> {
    let pid = std::process::id();
    let identity = process_identity(pid).ok_or("Could not identify the running desktop")?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut helper = command("/proc/self/exe")
        .arg("--development-restart-helper")
        .arg(pid.to_string())
        .arg(identity)
        .arg(artifact)
        .arg(data_dir)
        .arg(cwd)
        .args(std::env::args_os().skip(1))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start restart helper: {e}"))?;
    let stdout = helper.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        let result = io::BufReader::new(stdout)
            .read_line(&mut line)
            .map(|_| line);
        let _ = sender.send(result);
    });
    let ready =
        matches!(receiver.recv_timeout(Duration::from_secs(5)), Ok(Ok(line)) if line == "READY\n");
    if !ready {
        let _ = helper.kill();
        let _ = helper.wait();
        let _ = reader.join();
        return Err("Restart helper did not become ready; desktop remains running".into());
    }
    let _ = reader.join();
    // Reap a helper that times out while this desktop is still alive.
    std::thread::spawn(move || {
        let _ = helper.wait();
    });
    Ok(())
}

pub fn run_helper() -> bool {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args
        .get(1)
        .is_none_or(|arg| arg != "--development-restart-helper")
    {
        return false;
    }
    let Some(data_dir) = args.get(5).map(PathBuf::from) else {
        return true;
    };
    let result = (|| -> Result<(), String> {
        let pid = args
            .get(2)
            .and_then(|v| v.to_str())
            .and_then(|v| v.parse::<u32>().ok())
            .ok_or("Invalid original process")?;
        let identity = args
            .get(3)
            .and_then(|v| v.to_str())
            .ok_or("Missing process identity")?;
        let artifact = PathBuf::from(args.get(4).ok_or("Missing artifact")?);
        let cwd = args.get(6).ok_or("Missing working directory")?;
        digest(&artifact).map_err(|e| format!("Cannot read replacement: {e}"))?;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(data_dir.join("development/restart.log"))
            .map_err(|e| e.to_string())?;
        println!("READY");
        io::stdout().flush().map_err(|e| e.to_string())?;
        wait_for_exit(
            || process_identity(pid).as_deref() == Some(identity),
            Duration::from_secs(60),
        )
        .map_err(|e| e.to_string())?;
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(data_dir.join("development/restart.log"))
            .map_err(|e| e.to_string())?;
        command(artifact)
            .args(&args[7..])
            .current_dir(cwd)
            .env("TCODE_DATA_DIR", &data_dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|e| format!("Replacement launch failed: {e}"))?;
        Ok(())
    })();
    if let Err(error) = result {
        log_error(&data_dir, &error);
    }
    true
}
