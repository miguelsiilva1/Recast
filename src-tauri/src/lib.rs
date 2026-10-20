mod ffmpeg;
mod formats;

use formats::{Kind, Quality};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

struct Running {
    child: Mutex<Child>,
    cancelled: AtomicBool,
}

struct AppState {
    ffmpeg: Option<PathBuf>,
    encoders: HashSet<String>,
    jobs: Mutex<HashMap<String, Arc<Running>>>,
}

#[derive(Serialize)]
struct Status {
    ffmpeg: Option<String>,
}

#[derive(Serialize)]
struct FileInfo {
    path: String,
    name: String,
    size: u64,
    kind: Option<Kind>,
    targets: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    id: String,
    input: String,
    target: String,
    quality: Quality,
    out_dir: Option<String>,
}

#[derive(Clone, Serialize)]
struct Progress {
    id: String,
    percent: f64,
}

#[tauri::command]
fn status(state: State<'_, AppState>) -> Status {
    Status {
        ffmpeg: state.ffmpeg.as_ref().map(|p| p.display().to_string()),
    }
}

#[tauri::command]
fn inspect(state: State<'_, AppState>, paths: Vec<String>) -> Vec<FileInfo> {
    paths
        .into_iter()
        .filter(|p| Path::new(p).is_file())
        .map(|p| {
            let path = Path::new(&p);
            let kind = formats::kind_of(path);
            FileInfo {
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                size: path.metadata().map(|m| m.len()).unwrap_or(0),
                targets: kind.map(|k| formats::targets(k, &state.encoders)).unwrap_or_default(),
                kind,
                path: p,
            }
        })
        .collect()
}

#[tauri::command]
async fn convert(app: AppHandle, state: State<'_, AppState>, job: Job) -> Result<String, String> {
    let ffmpeg = state.ffmpeg.clone().ok_or("FFmpeg is not installed")?;
    let input = PathBuf::from(&job.input);
    let kind = formats::kind_of(&input).ok_or("Unsupported file type")?;
    if !formats::targets(kind, &state.encoders).contains(&job.target) {
        return Err(format!("Can't convert this file to {}", job.target));
    }
    let args = formats::output_args(kind, &job.target, job.quality, &state.encoders);
    let output = output_path(&input, job.out_dir.as_deref(), &job.target);

    let mut child = ffmpeg::command(&ffmpeg)
        .args(["-hide_banner", "-nostdin", "-y", "-i"])
        .arg(&input)
        .args(&args)
        .args(["-progress", "pipe:1", "-nostats"])
        .arg(&output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start FFmpeg: {e}"))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let running = Arc::new(Running { child: Mutex::new(child), cancelled: AtomicBool::new(false) });
    state.jobs.lock().unwrap().insert(job.id.clone(), running.clone());

    let id = job.id.clone();
    let out = output.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run(&app, &id, stdout, stderr, &running)?;
        Ok(out.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())?;

    state.jobs.lock().unwrap().remove(&job.id);
    if result.is_err() {
        let _ = std::fs::remove_file(&output);
    }
    result
}

#[tauri::command]
fn cancel(state: State<'_, AppState>, id: String) {
    if let Some(job) = state.jobs.lock().unwrap().get(&id) {
        job.cancelled.store(true, Ordering::Relaxed);
        let _ = job.child.lock().unwrap().kill();
    }
}

/// Streams ffmpeg's progress as events and waits for it to finish.
fn run(
    app: &AppHandle,
    id: &str,
    stdout: impl std::io::Read,
    stderr: impl std::io::Read + Send + 'static,
    running: &Running,
) -> Result<(), String> {
    let duration = Arc::new(Mutex::new(None::<f64>));
    let log = {
        let duration = duration.clone();
        std::thread::spawn(move || {
            let mut tail = VecDeque::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(d) = ffmpeg::parse_duration(&line) {
                    duration.lock().unwrap().get_or_insert(d);
                }
                if tail.len() == 4 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            Vec::from(tail).join("\n")
        })
    };

    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let Some(us) = line.strip_prefix("out_time_us=") else { continue };
        let total = *duration.lock().unwrap();
        if let (Ok(us), Some(total)) = (us.trim().parse::<f64>(), total) {
            if total > 0.0 {
                let percent = (us / 1e6 / total * 100.0).clamp(0.0, 99.9);
                let _ = app.emit("progress", Progress { id: id.to_owned(), percent });
            }
        }
    }

    let status = running.child.lock().unwrap().wait().map_err(|e| e.to_string())?;
    let log = log.join().unwrap_or_default();
    if running.cancelled.load(Ordering::Relaxed) {
        Err("Cancelled".into())
    } else if !status.success() {
        Err(if log.is_empty() { "FFmpeg failed".into() } else { log })
    } else {
        Ok(())
    }
}

/// `<dir>/<stem>.<ext>`, adding " (1)", " (2)"… so nothing gets overwritten.
fn output_path(input: &Path, out_dir: Option<&str>, ext: &str) -> PathBuf {
    let dir = out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or("output".into());
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    path
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run_app() {
    let ffmpeg = ffmpeg::locate();
    let encoders = ffmpeg.as_deref().map(ffmpeg::encoders).unwrap_or_default();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { ffmpeg, encoders, jobs: Mutex::default() })
        .invoke_handler(tauri::generate_handler![status, inspect, convert, cancel])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
