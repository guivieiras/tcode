//! Desktop media decoding and audio stay on one worker. GPUI receives only the
//! latest BGRA frame; libmpv owns playback timing, buffering and codec support.
use libloading::Library;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    ptr,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use tcode_traverse::file_stream::FileStream;

#[derive(Clone, Copy)]
pub(super) enum Command {
    Pause(bool),
    Seek(f64),
    Restart,
    Mute(bool),
}

#[derive(Default)]
pub(super) struct Snapshot {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub muted: bool,
    pub ended: bool,
    pub frame: Option<Frame>,
    pub error: Option<Error>,
}

pub(super) struct Frame {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(super) enum Error {
    Unavailable,
    Playback(String),
}

pub(super) struct Player {
    commands: mpsc::Sender<Command>,
    pub snapshot: Arc<Mutex<Snapshot>>,
}

impl Player {
    pub fn new(stream: FileStream) -> (Self, async_channel::Receiver<()>) {
        let (commands, receiver) = mpsc::channel();
        let (notify, updates) = async_channel::bounded(1);
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let state = snapshot.clone();
        std::thread::spawn(move || {
            let result = run(stream.url(), receiver, &state, &notify);
            if let Err(error) = result {
                state.lock().unwrap().error = Some(error);
                let _ = notify.try_send(());
            }
            // The authenticated endpoint outlives the decoder, including shutdown.
            drop(stream);
        });
        (Self { commands, snapshot }, updates)
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

// The following structs and signatures follow libmpv's stable client/render ABI.
#[repr(C)]
struct Param {
    kind: c_int,
    data: *mut c_void,
}
#[repr(C)]
struct Event {
    kind: c_int,
    error: c_int,
    id: u64,
    data: *mut c_void,
}
#[repr(C)]
struct Property {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}
#[repr(C)]
struct EndFile {
    reason: c_int,
    error: c_int,
}

macro_rules! api {
    ($($name:ident: $ty:ty),+ $(,)?) => {
        struct Api { $($name: $ty,)+ _library: Library }
        impl Api {
            unsafe fn from_library(library: Library) -> Result<Self, Error> {
                // SAFETY: these exact signatures are declared in mpv/client.h and render.h.
                Ok(Self { $($name: unsafe { *library.get::<$ty>(concat!(stringify!($name), "\0").as_bytes()).map_err(|_| Error::Unavailable)? },)+ _library: library })
            }
        }
    };
}
api! {
    mpv_create: unsafe extern "C" fn() -> *mut c_void,
    mpv_initialize: unsafe extern "C" fn(*mut c_void) -> c_int,
    mpv_terminate_destroy: unsafe extern "C" fn(*mut c_void),
    mpv_set_option_string: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    mpv_command_async: unsafe extern "C" fn(*mut c_void, u64, *const *const c_char) -> c_int,
    mpv_observe_property: unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int) -> c_int,
    mpv_wait_event: unsafe extern "C" fn(*mut c_void, f64) -> *const Event,
    mpv_error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    mpv_render_context_create: unsafe extern "C" fn(*mut *mut c_void, *mut c_void, *mut Param) -> c_int,
    mpv_render_context_update: unsafe extern "C" fn(*mut c_void) -> u64,
    mpv_render_context_render: unsafe extern "C" fn(*mut c_void, *mut Param) -> c_int,
    mpv_render_context_free: unsafe extern "C" fn(*mut c_void),
}

impl Api {
    fn load() -> Result<Self, Error> {
        #[cfg(target_os = "linux")]
        let names = ["libmpv.so.2", "libmpv.so.1"];
        #[cfg(target_os = "macos")]
        let names = [
            "libmpv.2.dylib",
            "/opt/homebrew/lib/libmpv.2.dylib",
            "/usr/local/lib/libmpv.2.dylib",
        ];
        #[cfg(target_os = "windows")]
        let names = ["libmpv-2.dll", "mpv-2.dll"];
        for name in names {
            // SAFETY: load the platform media library and retain it until every handle is freed.
            if let Ok(library) = unsafe { Library::new(name) } {
                return unsafe { Self::from_library(library) };
            }
        }
        Err(Error::Unavailable)
    }
    fn check(&self, code: c_int) -> Result<(), Error> {
        if code < 0 {
            // SAFETY: libmpv returns a static NUL-terminated string for every error code.
            let message = unsafe { CStr::from_ptr((self.mpv_error_string)(code)) };
            return Err(Error::Playback(message.to_string_lossy().into_owned()));
        }
        Ok(())
    }
}

struct Decoder {
    api: Api,
    handle: *mut c_void,
    render: *mut c_void,
}
impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the worker exclusively owns these handles; renderer must die first.
        unsafe {
            if !self.render.is_null() {
                (self.api.mpv_render_context_free)(self.render);
            }
            if !self.handle.is_null() {
                (self.api.mpv_terminate_destroy)(self.handle);
            }
        }
    }
}
impl Decoder {
    fn new(url: &str) -> Result<Self, Error> {
        let api = Api::load()?;
        // SAFETY: all handle operations below occur on this worker, with live C arguments.
        unsafe {
            let handle = (api.mpv_create)();
            if handle.is_null() {
                return Err(Error::Playback("Could not create media player".into()));
            }
            let mut player = Self {
                api,
                handle,
                render: ptr::null_mut(),
            };
            for (key, value) in [
                (c"vo", c"libmpv"),
                (c"config", c"no"),
                (c"terminal", c"no"),
                (c"keep-open", c"yes"),
                (c"osc", c"no"),
                (c"osd-level", c"0"),
            ] {
                player.api.check((player.api.mpv_set_option_string)(
                    handle,
                    key.as_ptr(),
                    value.as_ptr(),
                ))?;
            }
            player.api.check((player.api.mpv_initialize)(handle))?;
            for (id, name, format) in [
                (1, c"time-pos", 5),
                (2, c"duration", 5),
                (3, c"pause", 3),
                (4, c"mute", 3),
                (5, c"eof-reached", 3),
                (6, c"video-params/aspect", 5),
            ] {
                player.api.check((player.api.mpv_observe_property)(
                    handle,
                    id,
                    name.as_ptr(),
                    format,
                ))?;
            }
            let mut params = [
                Param {
                    kind: 1,
                    data: c"sw".as_ptr().cast_mut().cast(),
                },
                Param {
                    kind: 0,
                    data: ptr::null_mut(),
                },
            ];
            player.api.check((player.api.mpv_render_context_create)(
                &mut player.render,
                handle,
                params.as_mut_ptr(),
            ))?;
            player.command(&["loadfile", url])?;
            Ok(player)
        }
    }
    fn command(&self, args: &[&str]) -> Result<(), Error> {
        let args: Vec<_> = args.iter().map(|s| CString::new(*s).unwrap()).collect();
        let mut pointers: Vec<_> = args.iter().map(|s| s.as_ptr()).collect();
        pointers.push(ptr::null());
        // SAFETY: mpv_command_async copies the terminated argument array before returning.
        // Unlike synchronous commands, this is safe on the render thread.
        self.api
            .check(unsafe { (self.api.mpv_command_async)(self.handle, 0, pointers.as_ptr()) })
    }
    fn apply(&self, command: Command) -> Result<(), Error> {
        match command {
            Command::Pause(value) => {
                self.command(&["set", "pause", if value { "yes" } else { "no" }])
            }
            Command::Mute(value) => {
                self.command(&["set", "mute", if value { "yes" } else { "no" }])
            }
            Command::Seek(seconds) => {
                self.command(&["seek", &seconds.max(0.).to_string(), "absolute+exact"])
            }
            Command::Restart => {
                self.apply(Command::Seek(0.))?;
                self.apply(Command::Pause(false))
            }
        }
    }
    fn poll(&self, state: &mut Snapshot, aspect: &mut f64) -> Result<bool, Error> {
        let mut changed = false;
        // SAFETY: event payloads remain valid until the next mpv_wait_event call;
        // each cast is guarded by its event ID and requested property format.
        unsafe {
            loop {
                let event = &*(self.api.mpv_wait_event)(self.handle, 0.);
                if event.kind == 0 {
                    break;
                }
                self.api.check(event.error)?;
                if event.kind == 7 && !event.data.is_null() {
                    let end = &*event.data.cast::<EndFile>();
                    self.api.check(end.error)?;
                }
                if event.kind != 22 || event.data.is_null() {
                    continue;
                }
                let property = &*event.data.cast::<Property>();
                if property.data.is_null() {
                    continue;
                }
                if property.format == 5 {
                    let value = *property.data.cast::<f64>();
                    match event.id {
                        1 => state.position = value,
                        2 => state.duration = value,
                        6 if value > 0. => *aspect = value,
                        _ => {}
                    }
                } else if property.format == 3 {
                    let value = *property.data.cast::<c_int>() != 0;
                    match event.id {
                        3 => state.paused = value,
                        4 => state.muted = value,
                        5 => state.ended = value,
                        _ => {}
                    }
                }
                changed = true;
            }
        }
        Ok(changed)
    }
    fn frame(&self, aspect: f64) -> Result<Option<Frame>, Error> {
        // SAFETY: the worker exclusively owns the renderer. Vec<u32> provides
        // the four-byte alignment required by bgr0; all parameters live through render.
        unsafe {
            if (self.api.mpv_render_context_update)(self.render) & 1 == 0 {
                return Ok(None);
            }
            let width = (720. * aspect).clamp(1., 960.) as u32;
            let height = (width as f64 / aspect).clamp(1., 720.) as u32;
            let mut size = [width as c_int, height as c_int];
            let mut stride = width as usize * 4;
            let mut pixels = vec![0u32; width as usize * height as usize];
            let mut params = [
                Param {
                    kind: 17,
                    data: size.as_mut_ptr().cast(),
                },
                Param {
                    kind: 18,
                    data: c"bgr0".as_ptr().cast_mut().cast(),
                },
                Param {
                    kind: 19,
                    data: (&mut stride as *mut usize).cast(),
                },
                Param {
                    kind: 20,
                    data: pixels.as_mut_ptr().cast(),
                },
                Param {
                    kind: 0,
                    data: ptr::null_mut(),
                },
            ];
            self.api.check((self.api.mpv_render_context_render)(
                self.render,
                params.as_mut_ptr(),
            ))?;
            let bytes = pixels
                .into_iter()
                .flat_map(|pixel| {
                    let mut b = pixel.to_ne_bytes();
                    b[3] = 255;
                    b
                })
                .collect();
            Ok(Some(Frame {
                width,
                height,
                bytes,
            }))
        }
    }
}

fn run(
    url: &str,
    commands: mpsc::Receiver<Command>,
    state: &Mutex<Snapshot>,
    notify: &async_channel::Sender<()>,
) -> Result<(), Error> {
    let decoder = Decoder::new(url)?;
    let mut aspect = 16. / 9.;
    loop {
        match commands.recv_timeout(Duration::from_millis(16)) {
            Ok(command) => decoder.apply(command)?,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let changed = decoder.poll(&mut state.lock().unwrap(), &mut aspect)?;
        // Rendering may wait for media timing; never hold the UI snapshot lock here.
        if let Some(frame) = decoder.frame(aspect)? {
            state.lock().unwrap().frame = Some(frame);
        } else if !changed {
            continue;
        }
        let _ = notify.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, time::Instant};

    #[test]
    #[ignore = "Requires libmpv and TCODE_VIDEO_TEST_FILE pointing to a video at least 3 seconds long"]
    fn native_playback_controls_and_close_release_the_stream() {
        let path = PathBuf::from(std::env::var("TCODE_VIDEO_TEST_FILE").expect("video fixture"));
        let size = std::fs::metadata(&path).unwrap().len();
        let root =
            std::env::temp_dir().join(format!("tcode-video-controls-{}", std::process::id()));
        let host = tcode_runtime::pipe::spawn_host(
            tcode_services::store::SessionStore::open_at(root.clone()).unwrap(),
            tcode_runtime::pipe::HostServices::default(),
        )
        .unwrap();
        let stream = FileStream::new(host.link(), path, size, "video/mp4".into()).unwrap();
        let url = url::Url::parse(stream.url()).unwrap();
        let address = format!("127.0.0.1:{}", url.port().unwrap());
        let (player, _updates) = Player::new(stream);
        let until = |condition: &dyn Fn(&Snapshot) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                {
                    let state = player.snapshot.lock().unwrap();
                    assert!(state.error.is_none(), "{:?}", state.error);
                    if condition(&state) {
                        break;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "player did not reach the requested state"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        until(&|state| state.frame.is_some() && state.duration > 3. && state.position > 0.1);
        player.send(Command::Pause(true));
        until(&|state| state.paused);
        let position = player.snapshot.lock().unwrap().position;
        std::thread::sleep(Duration::from_millis(200));
        assert!((player.snapshot.lock().unwrap().position - position).abs() < 0.15);
        player.send(Command::Seek(2.));
        until(&|state| (state.position - 2.).abs() < 0.15 && state.paused);
        player.send(Command::Restart);
        until(&|state| state.position < 1. && !state.paused);
        player.send(Command::Mute(true));
        until(&|state| state.muted);
        drop(player);
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::net::TcpStream::connect(&address).is_ok() {
            assert!(
                Instant::now() < deadline,
                "closing the player must release its endpoint"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        host.shutdown_blocking().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
