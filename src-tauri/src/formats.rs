use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Image,
    Animated,
    Video,
    Audio,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    High,
    Balanced,
    Small,
}

impl Quality {
    fn pick<T>(self, high: T, balanced: T, small: T) -> T {
        match self {
            Quality::High => high,
            Quality::Balanced => balanced,
            Quality::Small => small,
        }
    }
}

pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" | "avif" | "heic" | "heif"
        | "tga" | "ico" => Kind::Image,
        "gif" | "apng" => Kind::Animated,
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" | "flv" | "wmv" | "mpg" | "mpeg" | "ts"
        | "mts" | "m2ts" | "3gp" | "ogv" => Kind::Video,
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "oga" | "opus" | "wma" | "aiff"
        | "aif" | "amr" => Kind::Audio,
        _ => return None,
    })
}

/// Output formats offered for each kind of input, in display order.
fn candidates(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Image => &["jpg", "png", "webp", "avif", "gif", "bmp", "tiff"],
        Kind::Animated => &["mp4", "webm", "gif", "webp", "mov", "png", "jpg"],
        Kind::Video => &[
            "mp4", "mov", "mkv", "webm", "avi", "gif", "webp", "mp3", "m4a", "wav", "flac",
            "opus", "ogg", "png", "jpg",
        ],
        Kind::Audio => &["mp3", "m4a", "wav", "flac", "opus", "ogg", "aiff"],
    }
}

fn supported(target: &str, kind: Kind, enc: &HashSet<String>) -> bool {
    let has = |e: &str| enc.contains(e);
    match target {
        "jpg" => has("mjpeg"),
        "png" => has("png"),
        "bmp" => has("bmp"),
        "tiff" => has("tiff"),
        "gif" => has("gif"),
        "webp" if kind == Kind::Image => has("libwebp"),
        "webp" => has("libwebp_anim"),
        "avif" => has("libsvtav1") || has("libaom-av1"),
        "mp4" | "mov" | "mkv" => has("libx264") && has("aac"),
        "webm" => has("libvpx-vp9") && has("libopus"),
        "avi" => has("mpeg4") && has("libmp3lame"),
        "mp3" => has("libmp3lame"),
        "m4a" => has("aac"),
        "wav" => has("pcm_s16le"),
        "aiff" => has("pcm_s16be"),
        "flac" => has("flac"),
        "opus" => has("libopus"),
        "ogg" => has("libvorbis"),
        _ => false,
    }
}

pub fn targets(kind: Kind, enc: &HashSet<String>) -> Vec<String> {
    candidates(kind)
        .iter()
        .filter(|t| supported(t, kind, enc))
        .map(|t| t.to_string())
        .collect()
}

// x264 and most still-image encoders need even dimensions.
const EVEN: &str = "scale=trunc(iw/2)*2:trunc(ih/2)*2";
// JPEG has no alpha: paint transparent areas white instead of black.
const FLATTEN: &str =
    "split[a][b];[a]format=yuv444p,drawbox=c=white:t=fill[bg];[bg][b]overlay=format=auto";

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// FFmpeg output options for converting an input of `kind` to `target`.
pub fn output_args(kind: Kind, target: &str, q: Quality, enc: &HashSet<String>) -> Vec<String> {
    let moving = kind != Kind::Image;
    let mut a = match target {
        "jpg" => args(&["-vf", FLATTEN, "-q:v", q.pick("2", "4", "8"), "-frames:v", "1"]),
        "png" | "bmp" | "tiff" => args(&["-frames:v", "1"]),
        "webp" if !moving => args(&[
            "-c:v", "libwebp", "-quality", q.pick("92", "80", "60"), "-frames:v", "1",
        ]),
        "webp" => {
            let (fps, w) = q.pick((20, 960), (15, 640), (10, 480));
            let vf = format!("fps={fps},scale='min({w},iw)':-2:flags=lanczos");
            let mut a = args(&["-vf", &vf, "-c:v", "libwebp_anim", "-loop", "0"]);
            a.extend(args(&["-quality", q.pick("85", "70", "50"), "-an"]));
            a
        }
        "avif" if enc.contains("libsvtav1") => args(&[
            "-vf", EVEN, "-c:v", "libsvtav1", "-crf", q.pick("24", "32", "40"),
            "-preset", "8", "-pix_fmt", "yuv420p", "-frames:v", "1",
        ]),
        "avif" => args(&[
            "-vf", EVEN, "-c:v", "libaom-av1", "-still-picture", "1",
            "-crf", q.pick("24", "32", "40"), "-cpu-used", "6", "-frames:v", "1",
        ]),
        "gif" => {
            let palette = "split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse";
            if moving {
                let (fps, w) = q.pick((20, 960), (15, 640), (10, 480));
                let vf = format!("fps={fps},scale='min({w},iw)':-2:flags=lanczos,{palette}");
                args(&["-vf", &vf, "-loop", "0", "-an"])
            } else {
                args(&["-vf", palette, "-frames:v", "1"])
            }
        }
        "mp4" | "mov" | "mkv" => {
            let mut a = args(&[
                "-vf", EVEN, "-c:v", "libx264", "-crf", q.pick("18", "23", "28"),
                "-preset", "medium", "-pix_fmt", "yuv420p",
                "-c:a", "aac", "-b:a", q.pick("192k", "160k", "96k"), "-sn", "-dn",
            ]);
            if target != "mkv" {
                a.extend(args(&["-movflags", "+faststart"]));
            }
            a
        }
        "webm" => args(&[
            "-vf", EVEN, "-c:v", "libvpx-vp9", "-crf", q.pick("30", "34", "40"), "-b:v", "0",
            "-row-mt", "1", "-deadline", "good", "-cpu-used", "4", "-pix_fmt", "yuv420p",
            "-c:a", "libopus", "-b:a", q.pick("160k", "128k", "80k"), "-sn", "-dn",
        ]),
        "avi" => args(&[
            "-vf", EVEN, "-c:v", "mpeg4", "-q:v", q.pick("3", "5", "8"),
            "-c:a", "libmp3lame", "-q:a", "2", "-sn", "-dn",
        ]),
        "mp3" => args(&["-c:a", "libmp3lame", "-q:a", q.pick("0", "2", "5")]),
        "m4a" => args(&["-c:a", "aac", "-b:a", q.pick("256k", "192k", "128k")]),
        "wav" => args(&["-c:a", "pcm_s16le"]),
        "aiff" => args(&["-c:a", "pcm_s16be"]),
        "flac" => args(&["-c:a", "flac"]),
        "opus" => args(&["-c:a", "libopus", "-b:a", q.pick("160k", "128k", "64k")]),
        "ogg" => args(&["-c:a", "libvorbis", "-q:a", q.pick("7", "5", "3")]),
        _ => Vec::new(),
    };
    if matches!(target, "mp3" | "m4a" | "wav" | "aiff" | "flac" | "opus" | "ogg") {
        a.splice(0..0, args(&["-vn", "-sn", "-dn"]));
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg;
    use std::process::Command;

    /// Converts a generated sample of each kind to every supported target with the real ffmpeg.
    #[test]
    fn every_target_converts() {
        let Some(bin) = ffmpeg::locate() else {
            eprintln!("ffmpeg not installed, skipping");
            return;
        };
        let enc = ffmpeg::encoders(&bin);
        let dir = std::env::temp_dir().join("recast-test");
        std::fs::create_dir_all(&dir).unwrap();

        let samples: [(Kind, &str, &[&str]); 4] = [
            (Kind::Image, "in.png", &["-f", "lavfi", "-i", "testsrc=s=101x75", "-frames:v", "1"]),
            (Kind::Animated, "in.gif", &["-f", "lavfi", "-i", "testsrc=d=1:s=99x75:r=10"]),
            (Kind::Video, "in.mp4", &[
                "-f", "lavfi", "-i", "testsrc=d=1:s=160x120", "-f", "lavfi", "-i", "sine=d=1", "-shortest",
            ]),
            (Kind::Audio, "in.wav", &["-f", "lavfi", "-i", "sine=d=1"]),
        ];

        let mut failures = Vec::new();
        for (kind, name, source) in samples {
            let input = dir.join(name);
            let ok = Command::new(&bin).args(["-hide_banner", "-loglevel", "error", "-y"]).args(source).arg(&input).status().unwrap();
            assert!(ok.success(), "could not generate {name}");

            for target in targets(kind, &enc) {
                for q in [Quality::High, Quality::Small] {
                    let output = dir.join(format!("out-{name}.{target}"));
                    let out = Command::new(&bin)
                        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                        .arg(&input)
                        .args(output_args(kind, &target, q, &enc))
                        .arg(&output)
                        .output()
                        .unwrap();
                    let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
                    if !out.status.success() || size == 0 {
                        failures.push(format!("{name} -> {target}: {}", String::from_utf8_lossy(&out.stderr)));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
