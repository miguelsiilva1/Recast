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
use tauri::{AppHandle, Emitter, Manager, State};

struct Running {
    child: Mutex<Child>,
    output: PathBuf,
    cancelled: AtomicBool,
}

impl Running {
    fn stop(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let _ = self.child.lock().unwrap().kill();
    }
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
    let mut files = Vec::new();
    for p in paths {
        let path = PathBuf::from(p);
        if path.is_dir() {
            collect_media(&path, &mut files);
        } else if path.is_file() {
            files.push(path);
        }
    }
    files
        .into_iter()
        .map(|path| {
            let kind = formats::kind_of(&path);
            FileInfo {
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                size: path.metadata().map(|m| m.len()).unwrap_or(0),
                targets: kind.map(|k| formats::targets(k, &state.encoders)).unwrap_or_default(),
                kind,
                path: path.display().to_string(),
            }
        })
        .collect()
}

/// Supported files inside a dropped folder and its subfolders, skipping hidden entries.
fn collect_media(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        if path.is_dir() {
            collect_media(&path, files);
        } else if formats::kind_of(&path).is_some() {
            files.push(path);
        }
    }
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
    let output = reserve_output(&input, job.out_dir.as_deref(), &job.target)
        .map_err(|e| format!("Could not create output file: {e}"))?;

    // A renamed playlist (HLS, concat…) could make ffmpeg read other files or URLs,
    // so only allow plain local files and the demuxers our supported formats need.
    let mut child = ffmpeg::command(&ffmpeg)
        .args(["-hide_banner", "-nostdin", "-y"])
        .args(["-protocol_whitelist", "file", "-format_whitelist", formats::DEMUXERS, "-i"])
        .arg(&input)
        .args(&args)
        .args(["-progress", "pipe:1", "-nostats"])
        .arg(&output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            let _ = std::fs::remove_file(&output);
            format!("Could not start FFmpeg: {e}")
        })?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let running = Arc::new(Running {
        child: Mutex::new(child),
        output: output.clone(),
        cancelled: AtomicBool::new(false),
    });
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
        job.stop();
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
        Err(explain(&log))
    } else {
        Ok(())
    }
}

/// Turns ffmpeg's log tail into a readable reason, keeping the raw log after a blank line.
fn explain(log: &str) -> String {
    let has = |s: &str| log.contains(s);
    let reason = if has("Invalid data found") || has("moov atom not found") {
        "The file is damaged or isn't the format its extension says."
    } else if has("does not contain any stream") || has("matches no streams") {
        "The file has no track that can be converted to this format."
    } else if has("No space left") {
        "Not enough disk space."
    } else if has("Permission denied") {
        "Recast isn't allowed to read the file or write to the output folder."
    } else if has("No such file") {
        "The file was moved or deleted."
    } else {
        "FFmpeg couldn't convert this file."
    };
    if log.is_empty() { reason.into() } else { format!("{reason}\n\n{log}") }
}

/// Creates an empty `<dir>/<stem>.<ext>`, adding " (1)", " (2)"… so nothing gets overwritten.
/// `create_new` fails on anything already there, including symlinks, so ffmpeg only ever writes our own file.
fn reserve_output(input: &Path, out_dir: Option<&str>, ext: &str) -> std::io::Result<PathBuf> {
    let dir = out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());
    if !dir.is_dir() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "the output folder no longer exists"));
    }
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or("output".into());
    for n in 0.. {
        let name = if n == 0 { format!("{stem}.{ext}") } else { format!("{stem} ({n}).{ext}") };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
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
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Quitting mid-conversion would otherwise leave ffmpeg running and a half-written file behind.
            if let tauri::RunEvent::Exit = event {
                for job in app.state::<AppState>().jobs.lock().unwrap().values() {
                    job.stop();
                    let _ = job.child.lock().unwrap().wait();
                    let _ = std::fs::remove_file(&job.output);
                }
            }
        });
}
