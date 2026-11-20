//! yt-dlp 실행: 설치 확인, 영상 탐색(JSON).
//!
//! - 항상 `--ignore-config` 로 사용자 설정 파일(옵션 주입 가능)을 무시한다.
//! - URL 은 검증 후 `--` 뒤에 인자로만 넘긴다.
//! - 로그인 정보는 앱이 저장하지 않는다. yt-dlp 가 브라우저 쿠키 또는 사용자가 고른 cookies.txt 를 직접 읽는다.
//! - DRM·접근 제한은 우회하지 않는다 (yt-dlp 가 거부하면 그대로 오류로 보고).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::safety::{redact_text, redact_url, validate_url};

const EXE: &str = "yt-dlp.exe";
/// 한 페이지에서 가져올 최대 영상 수 (재생목록·채널 전체를 끌어오지 않도록)
pub const MAX_ENTRIES: u32 = 100;
/// 탐색 제한 시간
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(180);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 로그인이 필요한 페이지를 위한 인증 방법. 쿠키 내용 자체는 앱이 읽거나 저장하지 않는다.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UrlAuth {
    #[default]
    None,
    /// 브라우저에 로그인된 세션의 쿠키를 yt-dlp 가 직접 읽는다.
    Browser(Browser),
    /// 사용자가 내보낸 Netscape 형식 cookies.txt
    CookieFile(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Browser {
    Chrome,
    Edge,
    Firefox,
    Whale,
    Brave,
}

impl Browser {
    pub const ALL: [Self; 5] = [Self::Chrome, Self::Edge, Self::Firefox, Self::Whale, Self::Brave];

    fn id(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
            Self::Firefox => "firefox",
            Self::Whale => "whale",
            Self::Brave => "brave",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
            Self::Firefox => "Firefox",
            Self::Whale => "Whale",
            Self::Brave => "Brave",
        }
    }
}

impl UrlAuth {
    pub(super) fn args(&self) -> Vec<std::ffi::OsString> {
        match self {
            Self::None => Vec::new(),
            Self::Browser(b) => vec!["--cookies-from-browser".into(), b.id().into()],
            Self::CookieFile(p) => vec!["--cookies".into(), p.as_os_str().to_owned()],
        }
    }
}

/// 탐색으로 찾은 영상 하나.
#[derive(Debug, Clone, PartialEq)]
pub struct UrlVideo {
    /// 사용자가 입력한 페이지/영상 주소 (다시 받을 때 사용)
    pub source_url: String,
    /// 페이지에 영상이 여러 개면 그 안의 번호 (yt-dlp --playlist-items)
    pub playlist_index: Option<u32>,
    pub id: String,
    pub title: String,
    pub duration: Option<Duration>,
    pub ext: String,
    pub resolution: Option<String>,
    pub has_video: bool,
    pub is_live: bool,
    pub drm: bool,
    pub extractor: String,
    /// yt-dlp 가 고른 실제 미디어 주소 (길이를 모를 때 ffprobe 로 확인하는 데만 사용, 로그 금지)
    pub media_url: Option<String>,
    /// yt-dlp 대신 FFmpeg 가 직접 읽을 미디어 (플레이어 설정에서 찾은 MP4 등)
    pub direct: Option<DirectMedia>,
    /// 탐색에 필요했던 요청 옵션 (받을 때도 같게 사용)
    pub request: RequestOpts,
}

/// yt-dlp 요청 옵션: 다른 페이지 안에 들어 있던 영상(iframe)은 그 페이지를 Referer 로,
/// 브라우저가 아닌 요청을 막는 사이트(Cloudflare 등)는 브라우저 요청 흉내(impersonate)로.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RequestOpts {
    pub referer: Option<String>,
    pub impersonate: bool,
}

impl RequestOpts {
    pub(super) fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        // validate_url 을 통과한 값만 (공백·개행 불가 → 헤더 주입 방지)
        if let Some(r) = self.referer.as_deref().and_then(|r| validate_url(r).ok()) {
            args.push("--add-headers".to_owned());
            args.push(format!("Referer:{r}"));
        }
        if self.impersonate {
            args.push("--impersonate".to_owned());
            args.push("chrome".to_owned());
        }
        args
    }
}

/// FFmpeg 가 직접 읽는 미디어 주소와, 플레이어 페이지에서 재생할 때 브라우저가 보내는 Referer
#[derive(Debug, Clone, PartialEq)]
pub struct DirectMedia {
    pub url: String,
    pub referer: String,
}

impl UrlVideo {
    /// 목록 중복 방지용 키
    pub fn key(&self) -> String {
        format!("url:{}#{}", self.source_url, self.playlist_index.unwrap_or(0))
    }

    /// 변환할 수 없는 이유 (없으면 선택 가능)
    pub fn unsupported_reason(&self) -> Option<&'static str> {
        if self.drm {
            Some("DRM 으로 보호된 영상은 지원하지 않습니다")
        } else if self.is_live {
            Some("실시간 방송은 지원하지 않습니다")
        } else {
            None
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UrlError {
    #[error("URL 변환 구성 요소가 설치되어 있지 않습니다. 설정에서 설치하세요.")]
    NotInstalled,
    #[error("{0}")]
    InvalidUrl(#[from] super::safety::UrlInputError),
    #[error("영상 처리 프로그램을 실행하지 못했습니다: {source}")]
    Spawn {
        tool: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("이 주소에서 재생 가능한 영상을 찾지 못했습니다")]
    NoVideos,
    #[error("이 주소에서 영상을 찾을 수 없습니다")]
    Unsupported,
    #[error("로그인이 필요하거나 접근 권한이 없습니다. 「로그인」에서 로그인해 둔 브라우저를 고르거나, 안 되면 쿠키 파일을 지정해 보세요. ({0})")]
    AuthRequired(String),
    #[error("DRM·보안 플레이어로 보호된 영상이라 처리할 수 없습니다")]
    Drm,
    #[error("시간 초과: 응답이 너무 오래 걸립니다")]
    Timeout,
    #[error("영상을 받지 못했습니다: {0}")]
    Failed(String),
    #[error("오디오 추출 실패: {0}")]
    Extract(String),
    #[error("응답 해석 실패: {0}")]
    Parse(String),
    #[error("작업이 취소되었습니다")]
    Cancelled,
    #[error("입출력 오류: {0}")]
    Io(#[from] std::io::Error),
}

/// 찾은 yt-dlp 실행 파일
#[derive(Debug, Clone)]
pub struct YtDlp {
    exe: PathBuf,
}

impl YtDlp {
    /// yt-dlp 를 다음 순서로 찾는다: 앱이 설치한 폴더 → 실행 파일 폴더 → PATH → winget 링크 폴더.
    /// (실행 확인은 하지 않는다. yt-dlp.exe 는 시작이 느려 버전 확인은 [`Self::version`] 으로 따로 한다)
    pub fn locate(managed_dir: Option<&Path>) -> Result<Self, UrlError> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(d) = managed_dir {
            dirs.push(d.to_path_buf());
        }
        if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
            dirs.push(exe_dir.join("yt-dlp"));
            dirs.push(exe_dir);
        }
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft").join("WinGet").join("Links"));
        }
        dirs.into_iter()
            .map(|d| d.join(EXE))
            .find(|p| p.is_file())
            .map(|exe| Self { exe })
            .ok_or(UrlError::NotInstalled)
    }

    pub fn from_path(exe: PathBuf) -> Self {
        Self { exe }
    }

    pub(super) fn command(&self) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.stdin(Stdio::null());
        // 사용자 설정 파일(--exec 등 옵션 주입 가능)을 읽지 않는다
        cmd.args(["--ignore-config", "--no-warnings", "--no-color"]);
        // YouTube 는 JS 런타임이 필요하다. 기본(deno)이 없고 Node.js 가 있으면 그것을 쓰게 한다.
        if let Some(node) = js_runtime_fallback() {
            cmd.arg("--js-runtimes").arg(format!("node:{}", node.display()));
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }

    pub fn version(&self) -> Result<String, UrlError> {
        let mut cmd = self.command();
        cmd.arg("--version");
        let out = run_capture(cmd, &AtomicBool::new(false), Duration::from_secs(60))?;
        if !out.status.success() {
            return Err(UrlError::Failed(stderr_summary(&out.stderr)));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// 주소에서 영상 목록을 찾는다 (다운로드 없음). 영상이 여러 개면 모두 돌려준다.
    pub fn discover(&self, url: &str, auth: &UrlAuth, cancel: &AtomicBool) -> Result<Vec<UrlVideo>, UrlError> {
        self.discover_with(url, auth, &RequestOpts::default(), cancel)
    }

    pub fn discover_with(&self, url: &str, auth: &UrlAuth, opts: &RequestOpts, cancel: &AtomicBool) -> Result<Vec<UrlVideo>, UrlError> {
        let url = validate_url(url)?;
        tracing::info!(
            url = %redact_url(&url),
            auth = auth_kind(auth),
            referer = opts.referer.is_some(),
            impersonate = opts.impersonate,
            "영상 탐색 시작"
        );
        let mut cmd = self.command();
        cmd.args(["--dump-single-json", "--skip-download", "--socket-timeout", "30"])
            .args(["--playlist-end", &MAX_ENTRIES.to_string()])
            .args(auth.args())
            .args(opts.args())
            .arg("--")
            .arg(&url);
        let out = run_capture(cmd, cancel, DISCOVER_TIMEOUT)?;
        if !out.status.success() {
            return Err(classify_error(&out.stderr));
        }
        let mut videos = parse_discovery(&out.stdout, &url)?;
        for v in &mut videos {
            v.request = opts.clone();
        }
        if videos.is_empty() {
            return Err(UrlError::NoVideos);
        }
        tracing::info!(count = videos.len(), "영상 탐색 완료");
        Ok(videos)
    }
}

/// deno 가 없고 node.exe 가 PATH 에 있으면 그 경로 (한 번만 찾는다)
fn js_runtime_fallback() -> Option<&'static PathBuf> {
    static FOUND: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
            if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
                dirs.push(exe_dir);
            }
            if dirs.iter().any(|d| d.join("deno.exe").is_file()) {
                return None; // yt-dlp 가 기본으로 deno 를 쓴다
            }
            dirs.into_iter().map(|d| d.join("node.exe")).find(|p| p.is_file())
        })
        .as_ref()
}

pub(super) fn auth_kind(auth: &UrlAuth) -> &'static str {
    match auth {
        UrlAuth::None => "none",
        UrlAuth::Browser(_) => "browser-cookies",
        UrlAuth::CookieFile(_) => "cookie-file",
    }
}

pub(super) struct Captured {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: ExitStatus,
}

/// 프로세스를 실행해 출력을 모은다. 취소·시간 초과 시 프로세스를 종료한다.
pub(super) fn run_capture(mut cmd: Command, cancel: &AtomicBool, timeout: Duration) -> Result<Captured, UrlError> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| UrlError::Spawn { tool: "yt-dlp", source })?;
    let out_t = child.stdout.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });
    let err_t = child.stderr.take().map(|s| spawn_tail_reader(s, 16 * 1024));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.load(Ordering::Relaxed) {
            kill(&mut child);
            return Err(UrlError::Cancelled);
        }
        if started.elapsed() > timeout {
            kill(&mut child);
            return Err(UrlError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stdout = out_t.and_then(|t| t.join().ok()).unwrap_or_default();
    let stderr = err_t.and_then(|t| t.join().ok()).unwrap_or_default();
    Ok(Captured { stdout, stderr, status })
}

/// 끝부분만 보관하며 읽는 스레드 (파이프가 가득 차 프로세스가 멈추지 않게)
pub(super) fn spawn_tail_reader(mut s: impl Read + Send + 'static, keep: usize) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = s.read(&mut buf) {
            if n == 0 {
                break;
            }
            kept.extend_from_slice(&buf[..n]);
            if kept.len() > keep * 2 {
                kept.drain(..kept.len() - keep);
            }
        }
        kept
    })
}

pub(super) fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// stderr 의 ERROR 줄을 중심으로 요약하고 URL 토큰을 가린다.
pub(super) fn stderr_summary(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let pick = lines
        .iter()
        .rev()
        .find(|l| l.starts_with("ERROR"))
        .or(lines.last())
        .copied()
        .unwrap_or("알 수 없는 오류");
    let msg = pick.trim_start_matches("ERROR:").trim();
    let msg: String = msg.chars().take(300).collect();
    redact_text(&msg)
}

pub(super) fn classify_error(stderr: &[u8]) -> UrlError {
    let summary = stderr_summary(stderr);
    let lower = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if lower.contains("drm") {
        UrlError::Drm
    } else if lower.contains("unsupported url") {
        UrlError::Unsupported
    } else if lower.contains("http error 401")
        || lower.contains("http error 403")
        || lower.contains("login")
        || lower.contains("sign in")
        || lower.contains("cookies")
        || lower.contains("authentication")
    {
        UrlError::AuthRequired(summary)
    } else {
        UrlError::Failed(summary)
    }
}

// ---------------------------------------------------------------- JSON 해석

fn parse_discovery(json: &[u8], source_url: &str) -> Result<Vec<UrlVideo>, UrlError> {
    let root: Value = serde_json::from_slice(json).map_err(|e| UrlError::Parse(e.to_string()))?;
    let is_playlist = root.get("_type").and_then(Value::as_str) == Some("playlist");
    if !is_playlist {
        return Ok(vec![video_from(&root, source_url, None)]);
    }
    let entries = root.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for (pos, entry) in entries.iter().enumerate() {
        if entry.is_null() {
            continue; // 접근할 수 없는 항목
        }
        let index = entry
            .get("playlist_index")
            .and_then(Value::as_u64)
            .and_then(|i| u32::try_from(i).ok())
            .unwrap_or(pos as u32 + 1);
        out.push(video_from(entry, source_url, Some(index)));
    }
    Ok(out)
}

fn video_from(v: &Value, source_url: &str, playlist_index: Option<u32>) -> UrlVideo {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let title = s("title").or_else(|| s("fulltitle")).unwrap_or_else(|| "제목 없음".to_owned());
    let duration = v
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(Duration::from_secs_f64);
    let resolution = s("resolution").filter(|r| r != "audio only").or_else(|| {
        match (v.get("width").and_then(Value::as_u64), v.get("height").and_then(Value::as_u64)) {
            (Some(w), Some(h)) => Some(format!("{w}x{h}")),
            (None, Some(h)) => Some(format!("{h}p")),
            _ => None,
        }
    });
    let vcodec = s("vcodec");
    let has_video = match vcodec.as_deref() {
        Some("none") => false,
        _ => s("resolution").as_deref() != Some("audio only"),
    };
    let is_live = v.get("is_live").and_then(Value::as_bool).unwrap_or(false)
        || s("live_status").as_deref() == Some("is_live");
    let drm = v.get("_has_drm").and_then(Value::as_bool).unwrap_or(false)
        || v.get("formats").and_then(Value::as_array).is_some_and(|fs| {
            !fs.is_empty() && fs.iter().all(|f| f.get("has_drm").and_then(Value::as_bool).unwrap_or(false))
        });
    let protocol = s("protocol").unwrap_or_default();
    let ext = match s("ext") {
        Some(e) if protocol.contains("m3u8") => format!("{e} (HLS)"),
        Some(e) => e,
        None if protocol.contains("m3u8") => "HLS".to_owned(),
        None => "-".to_owned(),
    };
    UrlVideo {
        source_url: source_url.to_owned(),
        playlist_index,
        id: s("id").unwrap_or_default(),
        title,
        duration,
        ext,
        resolution,
        has_video,
        is_live,
        drm,
        extractor: s("extractor_key").or_else(|| s("extractor")).unwrap_or_default(),
        media_url: s("url").filter(|u| u.starts_with("http://") || u.starts_with("https://")),
        direct: None,
        request: RequestOpts::default(),
    }
}

/// 길이를 모르는 영상(주로 직접 MP4 주소)을 ffprobe 로 보완한다. 로그인이 필요한 주소는 실패해도 그냥 둔다.
pub fn fill_missing_durations(videos: &mut [UrlVideo], tools: &crate::media::FfmpegTools, cancel: &AtomicBool) {
    for v in videos.iter_mut().filter(|v| v.duration.is_none()) {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let referer = v.direct.as_ref().map(|d| d.referer.clone()).or_else(|| v.request.referer.clone());
        if let Some(url) = v.media_url.clone() {
            v.duration = tools.probe_url_duration(&url, referer.as_deref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_video() {
        let json = br#"{"id":"a","title":"lecture","duration":125.5,"ext":"mp4","width":1280,"height":720,"vcodec":"avc1","extractor_key":"Generic","protocol":"https"}"#;
        let v = parse_discovery(json, "https://x/a.mp4").expect("parse");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].title, "lecture");
        assert_eq!(v[0].playlist_index, None);
        assert_eq!(v[0].resolution.as_deref(), Some("1280x720"));
        assert_eq!(v[0].duration, Some(Duration::from_millis(125_500)));
        assert!(v[0].has_video && v[0].unsupported_reason().is_none());
    }

    #[test]
    fn parses_playlist_and_flags() {
        let json = br#"{"_type":"playlist","entries":[
            {"id":"1","title":"one","ext":"mp4","resolution":"640x360","protocol":"m3u8_native"},
            null,
            {"id":"2","title":"two","is_live":true,"playlist_index":3},
            {"id":"3","title":"three","_has_drm":true,"resolution":"audio only","vcodec":"none"}
        ]}"#;
        let v = parse_discovery(json, "https://lms/page").expect("parse");
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].playlist_index, Some(1));
        assert_eq!(v[0].ext, "mp4 (HLS)");
        assert_eq!(v[1].playlist_index, Some(3));
        assert!(v[1].unsupported_reason().is_some());
        assert!(v[2].drm && !v[2].has_video);
        assert_ne!(v[0].key(), v[1].key());
    }

    #[test]
    fn classifies_errors_and_hides_tokens() {
        let e = classify_error(b"ERROR: [generic] Unsupported URL: https://x/y?k=SECRET");
        assert!(matches!(e, UrlError::Unsupported));
        let e = classify_error(b"ERROR: HTTP Error 403: Forbidden https://cdn/a.m3u8?token=SECRET");
        match e {
            UrlError::AuthRequired(msg) => assert!(!msg.contains("SECRET")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(classify_error(b"ERROR: This video is DRM protected"), UrlError::Drm));
    }

    #[test]
    fn auth_args_never_contain_cookie_values() {
        assert!(UrlAuth::None.args().is_empty());
        assert_eq!(UrlAuth::Browser(Browser::Edge).args(), vec!["--cookies-from-browser", "edge"]);
        let a = UrlAuth::CookieFile(PathBuf::from(r"C:\내 쿠키\cookies.txt")).args();
        assert_eq!(a[0], "--cookies");
    }
}
