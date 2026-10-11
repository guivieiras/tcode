//! Native client-owned APK transfers. No page owns a task or partially downloaded file.
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tcode_client::{HostLink, host::ApkDownloadState};
use tcode_protocol::{DevelopmentArtifact, MAX_FILE_RANGE_BYTES, Query, QueryResponse};
use tokio::io::AsyncWriteExt as _;

use crate::identity::encode_hex;

#[derive(Default)]
pub(crate) struct Downloads(Mutex<HashMap<(String, String), ApkDownloadState>>);
impl Downloads {
    pub fn state(&self, host: &str, build: &str) -> ApkDownloadState {
        self.0
            .lock()
            .unwrap()
            .get(&(host.into(), build.into()))
            .cloned()
            .unwrap_or_default()
    }
    fn set(&self, host: &str, build: &str, state: ApkDownloadState) {
        self.0
            .lock()
            .unwrap()
            .insert((host.into(), build.into()), state);
    }
    pub fn start(
        self: &Arc<Self>,
        root: PathBuf,
        host: String,
        link: HostLink,
        artifact: DevelopmentArtifact,
    ) -> async_channel::Receiver<Result<(), String>> {
        let (sender, receiver) = async_channel::bounded(1);
        let key = (host.clone(), artifact.build_id.clone());
        {
            let mut states = self.0.lock().unwrap();
            if matches!(states.get(&key), Some(ApkDownloadState::Downloading { .. })) {
                let _ = sender.try_send(Err("APK download is already running".into()));
                return receiver;
            }
            states.insert(
                key,
                ApkDownloadState::Downloading {
                    received: 0,
                    total: artifact.size,
                },
            );
        }
        let downloads = self.clone();
        // Detached on the Traverse runtime: the transfer outlives its requester.
        crate::runtime().spawn(async move {
            let result = transfer(&root, &host, &link, &artifact, |received| {
                downloads.set(
                    &host,
                    &artifact.build_id,
                    ApkDownloadState::Downloading {
                        received,
                        total: artifact.size,
                    },
                );
            })
            .await;
            downloads.set(
                &host,
                &artifact.build_id,
                match &result {
                    Ok(()) => ApkDownloadState::Ready,
                    Err(error) => ApkDownloadState::Failed(error.clone()),
                },
            );
            let _ = sender.try_send(result);
        });
        receiver
    }
}

pub(crate) fn cache_path(root: &Path, host: &str, build: &str) -> PathBuf {
    let key = encode_hex(&Sha256::digest(format!("{host}\0{build}").as_bytes()));
    root.join("apks").join(format!("{key}.apk"))
}

struct Partial(PathBuf);
impl Drop for Partial {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn transfer(
    root: &Path,
    host: &str,
    link: &HostLink,
    artifact: &DevelopmentArtifact,
    mut progress: impl FnMut(u64),
) -> Result<(), String> {
    let path = cache_path(root, host, &artifact.build_id);
    let parent = path.parent().unwrap();
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|e| e.to_string())?;
    }
    // Cache hits are verified too, including after Android process termination.
    let check = path.clone();
    let expected_id = artifact.build_id.clone();
    let expected_size = artifact.size;
    let cached = tokio::task::spawn_blocking(move || {
        use std::io::Read as _;
        let Ok(mut file) = std::fs::File::open(check) else {
            return false;
        };
        if file
            .metadata()
            .map_or(true, |meta| meta.len() != expected_size)
        {
            return false;
        }
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => hash.update(&buffer[..count]),
                Err(_) => return false,
            }
        }
        encode_hex(&hash.finalize()) == expected_id
    })
    .await
    .unwrap_or(false);
    if cached {
        progress(artifact.size);
        return Ok(());
    }
    let partial = Partial(path.with_extension("part"));
    let mut file = tokio::fs::File::create(&partial.0)
        .await
        .map_err(|e| e.to_string())?;
    let mut offset = 0;
    let mut hash = Sha256::new();
    while offset < artifact.size {
        let length = (artifact.size - offset).min(MAX_FILE_RANGE_BYTES as u64) as u32;
        let response = link
            .query(Query::ReadFileRange {
                path: artifact.path.clone(),
                offset,
                length,
                expected_size: artifact.size,
            })
            .await
            .map_err(|e| e.message)?;
        let QueryResponse::FileBytes(bytes) = response else {
            return Err("Unexpected APK range response".into());
        };
        if bytes.len() != length as usize {
            return Err("APK transfer was interrupted: incomplete chunk".into());
        }
        file.write_all(&bytes).await.map_err(|e| e.to_string())?;
        hash.update(&bytes);
        offset += bytes.len() as u64;
        progress(offset);
    }
    file.flush().await.map_err(|e| e.to_string())?;
    file.sync_all().await.map_err(|e| e.to_string())?;
    if offset != artifact.size || encode_hex(&hash.finalize()) != artifact.build_id {
        return Err("APK size or SHA-256 does not match the host build".into());
    }
    drop(file);
    tokio::fs::rename(&partial.0, path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tcode_protocol::{
        ClientPayload, HostMessage, ProtocolError, decode_client_line, encode_line,
    };

    fn host(bytes: Vec<u8>, fail_after_first: bool) -> HostLink {
        let (to_host, requests) = async_channel::unbounded::<String>();
        let (responses, from_host) = async_channel::unbounded::<String>();
        crate::runtime().spawn(async move {
            while let Ok(line) = requests.recv().await {
                let message = decode_client_line(&line).unwrap();
                if let ClientPayload::Query(Query::ReadFileRange { offset, length, .. }) =
                    message.payload
                {
                    let result = if fail_after_first && offset > 0 {
                        Err(ProtocolError {
                            code: "disconnected".into(),
                            message: "Connection lost".into(),
                        })
                    } else {
                        Ok(QueryResponse::FileBytes(
                            bytes[offset as usize..offset as usize + length as usize].to_vec(),
                        ))
                    };
                    if responses
                        .send(
                            encode_line(&HostMessage::QueryResult {
                                id: message.id,
                                result,
                            })
                            .unwrap(),
                        )
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        let link = HostLink::new(to_host, from_host);
        let pump = link.clone();
        crate::runtime().spawn(async move { pump.pump().await });
        link
    }

    #[test]
    fn multichunk_download_outlives_requester_and_bad_transfers_remove_partial_files() {
        crate::block_on(async {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tmp")
                .join(format!("apk-download-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let bytes: Vec<_> = (0..MAX_FILE_RANGE_BYTES * 2 + 17)
                .map(|i| (i % 251) as u8)
                .collect();
            let id = encode_hex(&Sha256::digest(&bytes));
            let artifact = DevelopmentArtifact {
                build_id: id.clone(),
                path: "app.apk".into(),
                size: bytes.len() as u64,
                checkout: "checkout".into(),
                completed_at: 1,
            };
            let downloads = Arc::new(Downloads::default());
            let link = host(bytes.clone(), false);
            let requester = downloads.start(
                root.clone(),
                "good-host".into(),
                link.clone(),
                artifact.clone(),
            );
            drop(requester);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while downloads.state("good-host", &id) != ApkDownloadState::Ready {
                assert!(std::time::Instant::now() < deadline);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert_eq!(
                std::fs::read(cache_path(&root, "good-host", &id)).unwrap(),
                bytes
            );
            link.close();
            for (name, data, interrupted) in [
                ("interrupted", bytes.clone(), true),
                ("mismatch", vec![0; bytes.len()], false),
            ] {
                let link = host(data, interrupted);
                let result = downloads
                    .start(root.clone(), name.into(), link.clone(), artifact.clone())
                    .recv()
                    .await
                    .unwrap();
                assert!(result.is_err());
                assert!(matches!(
                    downloads.state(name, &id),
                    ApkDownloadState::Failed(_)
                ));
                let path = cache_path(&root, name, &id);
                assert!(!path.exists());
                assert!(!path.with_extension("part").exists());
                link.close();
            }
            std::fs::remove_dir_all(root).unwrap();
        });
    }
}
