use super::*;
use tcode_client::host::{ApkDownloadState, ApkInstallerState};
use tcode_protocol::DevelopmentArtifact;

impl WorkspaceStore {
    pub(crate) fn development_command(
        &self,
        command: Command,
        cx: &mut App,
    ) -> Task<Result<(), tcode_protocol::ProtocolError>> {
        let host = self.host.clone();
        let connected = self.settings_connection_state().is_connected();
        cx.spawn(async move |_| {
            if !connected {
                return Err(tcode_protocol::ProtocolError {
                    code: "disconnected".into(),
                    message: "Reconnect before using Development controls".into(),
                });
            }
            host.command(command).await.map(|_| ())
        })
    }
    fn apk_host_key(&self) -> String {
        self.remote_host_id().unwrap_or("local").to_owned()
    }
    pub(crate) fn supports_apk_install(&self) -> bool {
        self.client_host
            .as_ref()
            .is_some_and(|host| host.supports_apk_install())
    }
    pub(crate) fn apk_download_state(&self, build: &str) -> ApkDownloadState {
        self.client_host
            .as_ref()
            .map_or_else(ApkDownloadState::default, |host| {
                host.apk_download_state(&self.apk_host_key(), build)
            })
    }
    pub(crate) fn download_apk(
        &self,
        artifact: DevelopmentArtifact,
        cx: &mut App,
    ) -> Task<Result<(), String>> {
        let client = self.client_host.clone();
        let link = self.host.clone();
        let key = self.apk_host_key();
        cx.spawn(async move |_| {
            let client = client.ok_or("APK download is unavailable")?;
            client.download_apk(key, link, artifact).await
        })
    }
    pub(crate) fn open_apk_installer(
        &self,
        build: String,
        cx: &mut App,
    ) -> Task<Result<ApkInstallerState, String>> {
        let client = self.client_host.clone();
        let key = self.apk_host_key();
        cx.spawn(async move |_| {
            let client = client.ok_or("APK installation is unavailable")?;
            client.open_apk_installer(key, build).await
        })
    }
}
