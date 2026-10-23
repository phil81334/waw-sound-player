#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::Manager;
use waw_sound_player::{
    audio::{Audio, Status},
    catalogue::{digest, CacheInfo, Catalogue, Index, Progress},
    sound::SoundInfo,
};

struct State {
    catalogue: Arc<Mutex<Option<Catalogue>>>,
    audio: Audio,
}

#[derive(Serialize)]
struct Startup {
    version: String,
    development: bool,
}

fn game_root() -> Result<PathBuf, String> {
    let root = if cfg!(debug_assertions) {
        std::env::var_os("WAW_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.local/test-root")
            })
    } else {
        std::env::current_exe()
            .map_err(|error| error.to_string())?
            .parent()
            .ok_or("Cannot locate the app folder.")?
            .to_path_buf()
    };
    if !root.join("CoDWaW.exe").is_file() {
        return Err("Put this executable in your World at War folder, beside CoDWaW.exe, then open it again.".into());
    }
    root.canonicalize().map_err(|error| error.to_string())
}

#[tauri::command]
fn startup(app: tauri::AppHandle, state: tauri::State<State>) -> Result<Startup, String> {
    let root = game_root()?;
    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("sounds-iwd-v1")
        .join(digest(root.to_string_lossy().as_bytes()));
    *state.catalogue.lock().map_err(|error| error.to_string())? = Some(Catalogue::new(root, cache));
    Ok(Startup {
        version: env!("CARGO_PKG_VERSION").into(),
        development: cfg!(debug_assertions),
    })
}

async fn catalogue_work<T: Send + 'static>(
    state: Arc<Mutex<Option<Catalogue>>>,
    work: impl FnOnce(&mut Catalogue) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = state.lock().map_err(|error| error.to_string())?;
        work(
            guard
                .as_mut()
                .ok_or("Open the app beside CoDWaW.exe first.")?,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn scan(
    force: bool,
    progress: tauri::ipc::Channel<Progress>,
    state: tauri::State<'_, State>,
) -> Result<Index, String> {
    let audio = state.audio.clone();
    tauri::async_runtime::spawn_blocking(move || audio.invalidate())
        .await
        .map_err(|error| error.to_string())??;
    catalogue_work(state.catalogue.clone(), move |catalogue| {
        catalogue.scan(force, |update| {
            let _ = progress.send(update);
        })
    })
    .await
}

#[derive(Serialize)]
struct Selection {
    generation: u64,
    info: SoundInfo,
}

#[tauri::command]
async fn select_sound(
    id: String,
    source: usize,
    play: bool,
    state: tauri::State<'_, State>,
) -> Result<Selection, String> {
    let audio = state.audio.clone();
    let reservation = audio.clone();
    let generation = tauri::async_runtime::spawn_blocking(move || reservation.invalidate())
        .await
        .map_err(|error| error.to_string())??;
    let sound = catalogue_work(state.catalogue.clone(), move |catalogue| {
        catalogue.load(&id, source)
    })
    .await?;
    let info = tauri::async_runtime::spawn_blocking(move || audio.load(sound, generation, play))
        .await
        .map_err(|error| error.to_string())??;
    Ok(Selection { generation, info })
}

#[tauri::command]
async fn transport(
    action: u32,
    value: f64,
    generation: u64,
    state: tauri::State<'_, State>,
) -> Result<Status, String> {
    let audio = state.audio.clone();
    tauri::async_runtime::spawn_blocking(move || audio.action(action, value, generation))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn playback_status(state: tauri::State<'_, State>) -> Result<Status, String> {
    let audio = state.audio.clone();
    tauri::async_runtime::spawn_blocking(move || audio.status())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn cache_info(state: tauri::State<'_, State>) -> Result<CacheInfo, String> {
    catalogue_work(state.catalogue.clone(), |catalogue| {
        Ok(catalogue.cache_info())
    })
    .await
}

#[tauri::command]
async fn clear_cache(state: tauri::State<'_, State>) -> Result<(), String> {
    catalogue_work(state.catalogue.clone(), |catalogue| catalogue.clear_cache()).await
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

fn main() {
    let profile = tempfile::Builder::new()
        .prefix("waw-sound-ui-")
        .tempdir()
        .expect("Cannot create the browser profile");
    let profile_path = profile.path().to_path_buf();
    tauri::Builder::default()
        .manage(State {
            catalogue: Arc::new(Mutex::new(None)),
            audio: Audio::new().expect("Cannot create the playback worker"),
        })
        .setup(move |app| {
            tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?
                .data_directory(profile_path)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            startup,
            scan,
            select_sound,
            transport,
            playback_status,
            cache_info,
            clear_cache,
            quit
        ])
        .run(tauri::generate_context!())
        .expect("Cannot start WaW Sound Player");
}
