use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use super::{AudioStreamInfo, MediaError, MediaInfo};

const FFMPEG_EXE: &str = "ffmpeg.exe";
const FFPROBE_EXE: &str = "ffprobe.exe";

/// Windows 에서 FFmpeg 실행 시 콘솔 창이 깜빡이지 않도록 한다.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 찾은 ffmpeg / ffprobe 실행 파일.
#[derive(Debug, Clone)]
pub struct FfmpegTools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    version: String,
}

impl FfmpegTools {
    /// FFmpeg 를 다음 순서로 찾는다.
    /// 1. 사용자가 설정한 폴더
    ///    (그다음) 앱이 자동 설치한 폴더 (%LOCALAPPDATA%\MediaTranscriber\data\ffmpeg)
    /// 2. 실행 파일 옆 `ffmpeg\` 폴더, 실행 파일 폴더 (배포용)
    /// 3. 현재 폴더의 `resources\ffmpeg` (개발용)
    /// 4. PATH
    /// 5. winget 링크 폴더 (설치 직후 PATH 가 갱신되지 않은 경우 대비)
    pub fn locate(user_dir: Option<&Path>) -> Result<Self, MediaError> {
        for dir in candidate_dirs(user_dir) {
            let ffmpeg = dir.join(FFMPEG_EXE);
            let ffprobe = dir.join(FFPROBE_EXE);
            if !(ffmpeg.is_file() && ffprobe.is_file()) {
                continue;
            }
            match read_version(&ffmpeg) {
                Ok(version) => {
                    tracing::info!(dir = %dir.display(), %version, "FFmpeg 발견");
                    return Ok(Self { ffmpeg, ffprobe, version });
                }
                Err(err) => {
                    tracing::warn!(dir = %dir.display(), error = %err, "FFmpeg 실행 확인 실패");
                }
            }
        }
        Err(MediaError::FfmpegNotFound)
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// 인코더 지원 여부 (예: "libmp3lame"). Cloud 업로드용 MP3 인코딩에 필요하다.
    pub fn has_encoder(&self, name: &str) -> bool {
        let output = base_command(&self.ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|line| line.split_whitespace().nth(1) == Some(name)),
            _ => false,
        }
    }

    pub(super) fn ffmpeg_command(&self) -> Command {
        base_command(&self.ffmpeg)
    }

    /// ffprobe 로 실제 미디어 정보를 검사한다. 확장자는 신뢰하지 않는다.
    pub fn probe(&self, path: &Path) -> Result<MediaInfo, MediaError> {
        let path = check_input_file(path)?;

        // 사용자 입력은 문자열 결합 없이 인자 배열로만 전달한다.
        let output = base_command(&self.ffprobe)
            .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", "-i"])
            .arg(&path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|source| MediaError::Spawn { tool: "ffprobe", source })?;

        if !output.status.success() {
            return Err(MediaError::InvalidMedia(stderr_summary(&output.stderr)));
        }
        parse_probe_json(&output.stdout)
    }
}

/// 입력 파일 존재/크기를 확인하고 절대 경로를 돌려준다.
pub(super) fn check_input_file(path: &Path) -> Result<PathBuf, MediaError> {
    let meta = match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return Err(MediaError::FileNotFound(path.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(MediaError::FileNotFound(path.to_path_buf()));
        }
        Err(e) => return Err(MediaError::Io(e)),
    };
    if meta.len() == 0 {
        return Err(MediaError::EmptyFile);
    }
    // 상대 경로나 '-' 로 시작하는 이름이 옵션으로 해석되지 않도록 절대 경로를 사용한다.
    Ok(std::path::absolute(path)?)
}

fn base_command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn candidate_dirs(user_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = user_dir {
        dirs.push(dir.to_path_buf());
    }
    if let Some(paths) = crate::config::AppPaths::resolve() {
        dirs.push(paths.ffmpeg_dir);
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        dirs.push(exe_dir.join("ffmpeg"));
        dirs.push(exe_dir);
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("resources").join("ffmpeg"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft").join("WinGet").join("Links"));
    }
    dirs
}

fn read_version(ffmpeg: &Path) -> Result<String, MediaError> {
    let output = base_command(ffmpeg)
        .arg("-version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|source| MediaError::Spawn { tool: "ffmpeg", source })?;
    if !output.status.success() {
        return Err(MediaError::DecodeFailed("ffmpeg -version 실패".to_owned()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(short_version(text.lines().next().unwrap_or_default()))
}

/// "ffmpeg version 9.0.2-full_build-www.gyan.dev Copyright ..." → "9.0.2"
fn short_version(first_line: &str) -> String {
    first_line
        .split_whitespace()
        .nth(2)
        .map(|v| v.split('-').next().unwrap_or(v))
        .filter(|v| !v.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

/// stderr 의 마지막 몇 줄을 사용자 표시용으로 요약한다.
pub(super) fn stderr_summary(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines().map(clean_ffmpeg_line).filter(|l| !l.is_empty()) {
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return "알 수 없는 오류".to_owned();
    }
    let start = lines.len().saturating_sub(3);
    lines[start..].join(" / ")
}

/// 사용자에게 의미 없는 부분을 걷어 낸다.
/// - `[mp3 @ 000001d25d773640] ` 같은 내부 구성 요소·주소 접두어
/// - `C:\...\파일.mp3: ` 같은 입력 경로 접두어 (파일명은 화면에 이미 보인다)
fn clean_ffmpeg_line(line: &str) -> String {
    let mut l = line.trim();
    while let Some(rest) = l.strip_prefix('[') {
        match rest.split_once("] ") {
            Some((tag, after)) if tag.contains(" @ ") => l = after.trim(),
            _ => break,
        }
    }
    if (l.contains('\\') || l.contains('/'))
        && let Some((_, msg)) = l.rsplit_once(": ")
    {
        l = msg.trim();
    }
    l.to_owned()
}

#[derive(Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    duration: Option<String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    format_name: Option<String>,
    duration: Option<String>,
}

fn parse_seconds(s: Option<&str>) -> Option<Duration> {
    let secs: f64 = s?.trim().parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

fn parse_probe_json(json: &[u8]) -> Result<MediaInfo, MediaError> {
    let probe: ProbeOutput = serde_json::from_slice(json)
        .map_err(|e| MediaError::InvalidMedia(format!("ffprobe 결과 해석 실패: {e}")))?;

    let format = probe
        .format
        .ok_or_else(|| MediaError::InvalidMedia("컨테이너 정보를 읽을 수 없습니다".to_owned()))?;

    let has_video = probe.streams.iter().any(|s| s.codec_type.as_deref() == Some("video"));
    let audio_stream = probe
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"))
        .ok_or(MediaError::NoAudioStream)?;

    let duration = parse_seconds(format.duration.as_deref())
        .or_else(|| parse_seconds(audio_stream.duration.as_deref()));

    Ok(MediaInfo {
        format_name: format.format_name.unwrap_or_default(),
        duration,
        has_video,
        audio: AudioStreamInfo {
            codec: audio_stream.codec_name.clone().unwrap_or_default(),
            sample_rate: audio_stream.sample_rate.as_deref().and_then(|s| s.parse().ok()),
            channels: audio_stream.channels,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_probe_json() {
        let json = br#"{
            "streams": [
                {"codec_type": "video", "codec_name": "h264"},
                {"codec_type": "audio", "codec_name": "aac", "sample_rate": "44100", "channels": 2}
            ],
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "12.500000"}
        }"#;
        let info = parse_probe_json(json).expect("parse");
        assert!(info.has_video);
        assert_eq!(info.audio.codec, "aac");
        assert_eq!(info.audio.sample_rate, Some(44100));
        assert_eq!(info.duration, Some(Duration::from_millis(12_500)));
    }

    #[test]
    fn no_audio_stream_is_error() {
        let json = br#"{"streams":[{"codec_type":"video"}],"format":{"format_name":"mp4"}}"#;
        assert!(matches!(parse_probe_json(json), Err(MediaError::NoAudioStream)));
    }

    #[test]
    fn version_is_shortened() {
        assert_eq!(short_version("ffmpeg version 9.0.2-full_build-www.gyan.dev Copyright"), "9.0.2");
        assert_eq!(short_version("ffmpeg version N-12345-gabc"), "N");
        assert_eq!(short_version(""), "unknown");
    }

    #[test]
    fn stderr_summary_keeps_last_lines() {
        assert_eq!(stderr_summary(b""), "알 수 없는 오류");
        assert_eq!(stderr_summary(b"a\n\nb\nc\nd\n"), "b / c / d");
    }

    #[test]
    fn ffmpeg_noise_is_removed() {
        let raw = "[mp3 @ 000001d25d773640] Failed to find two consecutive MPEG audio frames.\n\
                   C:\\Users\\me\\gui mix\\fake.mp3: Invalid data found when processing input\n";
        assert_eq!(
            stderr_summary(raw.as_bytes()),
            "Failed to find two consecutive MPEG audio frames. / Invalid data found when processing input"
        );
        assert_eq!(clean_ffmpeg_line("[in#0 @ 0x1] [mp3 @ 0x2] bad"), "bad");
        assert_eq!(clean_ffmpeg_line("[abc] keep"), "[abc] keep");
    }
}
