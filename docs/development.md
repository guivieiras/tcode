# Native development

Settings → Development builds the attached Linux desktop's source checkout.
Both desktop and Android require an initial manual update to expose these controls.
The feature uses the authenticated host connection and includes saved, uncommitted
changes. A build identifies the resulting artifact, not an exact source snapshot.

This topic starts from main with `topic/file-preview` as a prerequisite, fast-forwarded
to its existing tip before implementation. It reuses authenticated file-range queries.

Choose a Source checkout on the host filesystem. Rebuild runs
`cargo build --release --locked --bin tcode`; Build APK runs
`crates/android/host/build.sh --release`. Install prerequisites yourself if a build
reports missing tools. Only one build runs at a time; leaving the page or disconnecting
does not cancel it. Failed builds preserve the previous successful artifacts.

Restart desktop waits for a short-lived helper to be ready, shuts down providers and
terminals, flushes history, then launches the selected immutable artifact. Confirming
interruption happens on the requesting client. The helper waits up to 60 seconds and
never kills an old desktop or starts a competing host. Errors are recorded under
`<data-dir>/development/restart.log`. There is no automatic rollback; a launch failure
requires another way into the computer.

Android downloads APKs through the paired connection, verifies size and SHA-256,
and caches completed files by host and build. Open installer explicitly hands the
verified file to Android's PackageInstaller. Android may first request permission
to install unknown apps and then ask for installation confirmation. Opening that
confirmation does not mean installation succeeded. Desktop clients can copy the
APK's host path.

Build records and the latest two successful artifacts per target live under
`<data-dir>/development/`. The running executable is retained as well. Unreferenced
artifacts are pruned at host startup. Output retains the latest 64 KiB; an interrupted
host marks unfinished builds interrupted on its next start.
