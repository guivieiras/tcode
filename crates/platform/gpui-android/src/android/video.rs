//! Native player lifetime. Cancelling the future dismisses the Android dialog.
use futures::channel::oneshot;
use jni::{
    EnvUnowned, JavaVM, jni_sig, jni_str,
    objects::{JObject, JValue},
    sys::jlong,
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT: AtomicU64 = AtomicU64::new(1);
static PLAYERS: LazyLock<Mutex<HashMap<u64, oneshot::Sender<Result<(), String>>>>> =
    LazyLock::new(Default::default);

pub async fn play(url: String, title: String, error: String, close: String) -> Result<(), String> {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = oneshot::channel();
    PLAYERS.lock().insert(id, sender);
    let _guard = Player(id);
    dispatch(id, Some([url, title, error, close]));
    receiver
        .await
        .map_err(|_| "Video player was closed".to_string())?
}

struct Player(u64);
impl Drop for Player {
    fn drop(&mut self) {
        PLAYERS.lock().remove(&self.0);
        dispatch(self.0, None);
    }
}

fn finish(id: u64, result: Result<(), String>) {
    if let Some(sender) = PLAYERS.lock().remove(&id) {
        let _ = sender.send(result);
    }
}

fn dispatch(id: u64, arguments: Option<[String; 4]>) {
    let Some(app) = super::host::APP.lock().clone() else {
        finish(id, Err("Android activity is unavailable".into()));
        return;
    };
    let callback_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        // SAFETY: the AndroidApp retains the activity and its VM for this callback.
        let vm = unsafe { JavaVM::from_raw(callback_app.vm_as_ptr().cast()) };
        // The UI thread is already attached, so this only pushes a local frame.
        let result = vm
            .attach_current_thread(|env| -> jni::errors::Result<()> {
                // SAFETY: this is the live NativeActivity instance; do not delete its reference.
                let activity =
                    unsafe { JObject::from_raw(env, callback_app.activity_as_ptr().cast()) };
                let called = if let Some(arguments) = arguments {
                    let strings = arguments
                        .iter()
                        .map(|value| env.new_string(value))
                        .collect::<jni::errors::Result<Vec<_>>>()?;
                    env.call_method(
                        &activity,
                        jni_str!("gpuiPlayVideo"),
                        jni_sig!("(JLjava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"),
                        &[
                            JValue::Long(id as i64),
                            JValue::Object(strings[0].as_ref()),
                            JValue::Object(strings[1].as_ref()),
                            JValue::Object(strings[2].as_ref()),
                            JValue::Object(strings[3].as_ref()),
                        ],
                    )
                } else {
                    env.call_method(
                        &activity,
                        jni_str!("gpuiCloseVideo"),
                        jni_sig!("(J)V"),
                        &[JValue::Long(id as i64)],
                    )
                };
                called.inspect_err(|_| env.exception_clear())?;
                Ok(())
            })
            .map_err(|e| e.to_string());
        if let Err(error) = result {
            finish(id, Err(error));
        }
    }));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_tryanks_tcode_VideoPreview_nativeClosed<'local>(
    _env: EnvUnowned<'local>,
    _class: JObject<'local>,
    id: jlong,
) {
    finish(id as u64, Ok(()));
}
