//! A dedicated audio owner keeps COM, XAudio2 and all voice buffers on one thread.
use crate::sound::{Sound, SoundInfo};
use serde::Serialize;
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
};

#[repr(C)]
struct NativeStatus {
    position: f64,
    playing: u32,
    ended: u32,
    error: i32,
}

extern "C" {
    fn waw_audio_create() -> *mut c_void;
    fn waw_audio_destroy(player: *mut c_void);
    fn waw_audio_clear(player: *mut c_void);
    fn waw_audio_load(
        player: *mut c_void,
        format: *const u8,
        format_size: u32,
        data: *const u8,
        data_size: u32,
        packets: *const u32,
        packet_count: u32,
        frames: u32,
        block_frames: u32,
        rate: u32,
        start: bool,
    ) -> i32;
    fn waw_audio_action(player: *mut c_void, action: u32, value: f64) -> i32;
    fn waw_audio_status(player: *mut c_void) -> NativeStatus;
}

#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub generation: u64,
    pub loaded: bool,
    pub playing: bool,
    pub ended: bool,
    pub position: f64,
    pub error: Option<String>,
}

fn check(code: i32) -> Result<(), String> {
    if code < 0 {
        Err(format!("Windows audio error 0x{:08X}. Check your output device; this clip may use unsupported or damaged audio.", code as u32))
    } else {
        Ok(())
    }
}

struct Player {
    pointer: *mut c_void,
    generation: u64,
    loaded: bool,
}

impl Drop for Player {
    fn drop(&mut self) {
        unsafe {
            waw_audio_destroy(self.pointer);
        }
    }
}

impl Player {
    fn status(&self) -> Status {
        let state = unsafe { waw_audio_status(self.pointer) };
        Status {
            generation: self.generation,
            loaded: self.loaded,
            playing: state.playing != 0,
            ended: state.ended != 0,
            position: state.position,
            error: check(state.error).err(),
        }
    }
}

type Work = Box<dyn FnOnce(&mut Player) + Send>;

#[derive(Clone)]
pub struct Audio {
    sender: mpsc::Sender<Work>,
    pub generation: Arc<AtomicU64>,
}

impl Audio {
    pub fn new() -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel::<Work>();
        let (ready, initialized) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("sound-playback".into())
            .spawn(move || {
                let pointer = unsafe { waw_audio_create() };
                if pointer.is_null() {
                    let _ = ready.send(false);
                    return;
                }
                let mut player = Player {
                    pointer,
                    generation: 0,
                    loaded: false,
                };
                let _ = ready.send(true);
                for work in receiver {
                    work(&mut player);
                }
            })
            .map_err(|error| error.to_string())?;
        if !initialized.recv().map_err(|error| error.to_string())? {
            return Err("Cannot create the audio player.".into());
        }
        Ok(Self {
            sender,
            generation: Arc::new(AtomicU64::new(0)),
        })
    }

    fn call<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Player) -> T + Send + 'static,
    ) -> Result<T, String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(Box::new(move |player| {
                let _ = sender.send(work(player));
            }))
            .map_err(|_| "The audio worker stopped.".to_string())?;
        receiver
            .recv()
            .map_err(|_| "The audio worker stopped.".into())
    }

    pub fn invalidate(&self) -> Result<u64, String> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = self.generation.clone();
        self.call(move |player| {
            // Concurrent selections may enqueue their clears in a different order.
            if generation != latest.load(Ordering::SeqCst) {
                return;
            }
            unsafe {
                waw_audio_clear(player.pointer);
            }
            player.generation = generation;
            player.loaded = false;
        })?;
        Ok(generation)
    }

    pub fn load(&self, sound: Sound, generation: u64, start: bool) -> Result<SoundInfo, String> {
        let latest = self.generation.clone();
        self.call(move |player| {
            if generation != latest.load(Ordering::SeqCst) {
                return Err("Selection changed.".into());
            }
            let code = unsafe {
                waw_audio_load(
                    player.pointer,
                    sound.bytes[sound.format.clone()].as_ptr(),
                    sound.format.len() as u32,
                    sound.bytes[sound.data.clone()].as_ptr(),
                    sound.data.len() as u32,
                    sound.packets.as_ptr(),
                    sound.packets.len() as u32,
                    sound.frames,
                    sound.block_frames,
                    sound.info.sample_rate,
                    start,
                )
            };
            player.loaded = code >= 0;
            player.generation = generation;
            check(code)?;
            Ok(sound.info)
        })?
    }

    pub fn action(&self, action: u32, value: f64, generation: u64) -> Result<Status, String> {
        if !value.is_finite() {
            return Err("Invalid playback value.".into());
        }
        self.call(move |player| {
            if action < 4 && generation != player.generation {
                return Err("Selection changed.".into());
            }
            let code = unsafe { waw_audio_action(player.pointer, action, value) };
            check(code)?;
            Ok(player.status())
        })?
    }

    pub fn status(&self) -> Result<Status, String> {
        self.call(|player| player.status())
    }
}
