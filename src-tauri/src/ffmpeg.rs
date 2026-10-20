use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Builds a Command that won't flash a console window on Windows.
pub fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Finds a working ffmpeg: bundled sidecar first, then common install locations, then PATH.
pub fn locate() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut candidates = Vec::new();

    // Tauri places sidecars next to the app executable.
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join(exe));
    }
    // GUI apps on macOS don't inherit the shell PATH, so check the usual locations explicitly.
    for dir in [
        "/opt/homebrew/opt/ffmpeg-full/bin",
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/snap/bin",
    ] {
        candidates.push(Path::new(dir).join(exe));
    }
    candidates.push(PathBuf::from(exe));

    candidates.into_iter().find(|p| {
        command(p)
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// Names of the encoders this ffmpeg build was compiled with.
pub fn encoders(ffmpeg: &Path) -> HashSet<String> {
    let Ok(out) = command(ffmpeg).args(["-hide_banner", "-encoders"]).output() else {
        return HashSet::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("------"))
        .skip(1)
        .filter_map(|l| l.split_whitespace().nth(1).map(str::to_owned))
        .collect()
}

/// Parses "  Duration: 00:01:23.45, start: ..." into seconds.
pub fn parse_duration(line: &str) -> Option<f64> {
    let rest = line.trim_start().strip_prefix("Duration: ")?;
    let stamp = rest.split(',').next()?;
    let mut secs = 0.0;
    for part in stamp.split(':') {
        secs = secs * 60.0 + part.trim().parse::<f64>().ok()?;
    }
    Some(secs)
}
