//! 선택한 영상의 오디오만 추출해 임시 WAV 로 저장한다.
//!
//! **1차 (기본)**: yt-dlp(stdout) ──파이프──▶ ffmpeg(stdin) ──▶ 16 kHz · mono · PCM 16-bit WAV
//! - 영상 파일 전체를 디스크에 저장하지 않는다 (스트림을 받는 즉시 오디오로 변환).
//! - HLS(m3u8) · 오디오 전용 형식 · faststart MP4 는 모두 이 경로로 처리된다.
//!
//! **2차 (대체)**: 색인(moov)이 파일 끝에 있는 MP4 처럼 파이프로는 읽을 수 없는 형식이면
//! 임시 파일로 받은 뒤 오디오만 추출하고, 받은 파일은 즉시 지운다 (성공·실패·취소 모두).
//!
//! - 오디오만 있는 형식이 있으면 그것을 받는다 (`-f bestaudio/best`).
//! - 진행률은 ffmpeg `-progress` / yt-dlp 받은 바이트로 계산하고, 취소 시 프로세스를 모두 종료한다.
//! - 실패·취소 시 만들던 WAV 는 호출자의 임시 파일 가드가 지운다.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::safety::redact_url;
use super::ytdlp::{UrlAuth, UrlError, UrlVideo, YtDlp, auth_kind, classify_error, kill, spawn_tail_reader};
use crate::media::FfmpegTools;
use crate::utils::tempfile::TempDir;

/// 대체 경로에서 받기가 차지하는 진행률 비중 (나머지는 오디오 변환)
const DOWNLOAD_SHARE: f32 = 0.8;

pub fn extract_audio(
    ytdlp: &YtDlp,
    ffmpeg: &FfmpegTools,
    video: &UrlVideo,
    auth: &UrlAuth,
    out_wav: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<(), UrlError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(UrlError::Cancelled);
    }
    let url = super::safety::validate_url(&video.source_url)?;
    tracing::info!(
        url = %redact_url(&url),
        index = ?video.playlist_index,
        auth = auth_kind(auth),
        "영상 오디오 추출 시작"
    );
    // 진행률은 줄어들지 않게 (1차 시도에서 일부 보고한 뒤 2차로 넘어가도)
    let mut best = 0.0f32;
    let mut emit = |p: f32| {
        if p > best {
            best = p;
            on_progress(p);
        }
    };

    let outcome = match &video.direct {
        // 플레이어 설정에서 찾은 MP4: FFmpeg 가 직접 읽는다 (구간 요청으로 필요한 부분만, 영상 저장 없음)
        Some(direct) => {
            direct_extract(ffmpeg, direct, video.duration, out_wav, cancel, &mut emit)?;
            PipeOutcome::Done
        }
        None => pipe_extract(ytdlp, ffmpeg, video, auth, &url, out_wav, cancel, &mut emit)?,
    };
    match outcome {
        PipeOutcome::Done => {}
        PipeOutcome::NeedsFile(reason) => {
            tracing::info!(reason = %reason, "파이프로 읽을 수 없는 형식 - 임시 파일로 받아서 추출");
            let _ = std::fs::remove_file(out_wav);
            file_extract(ytdlp, ffmpeg, video, auth, &url, out_wav, cancel, &mut emit)?;
        }
    }
    let size = std::fs::metadata(out_wav).map(|m| m.len()).unwrap_or(0);
    // WAV 헤더(44바이트)만 있고 소리가 없으면 실패로 본다
    if size <= 1024 {
        return Err(UrlError::Extract("오디오 트랙이 없거나 비어 있습니다".to_owned()));
    }
    emit(1.0);
    tracing::info!(bytes = size, "영상 오디오 추출 완료");
    Ok(())
}

pub(super) enum PipeOutcome {
    Done,
    /// 받기는 됐지만 ffmpeg 가 파이프 입력을 해석하지 못함 → 파일로 다시 시도
    NeedsFile(String),
}

/// yt-dlp 명령: 선택한 영상 하나를 `output`(`-` 이면 표준 출력)으로 받는다.
/// 옵션(`extra` 포함)은 모두 `--` 앞에 둔다. `--` 뒤의 인자는 전부 주소로 해석된다.
fn ytdlp_command(
    ytdlp: &YtDlp,
    ffmpeg: &FfmpegTools,
    video: &UrlVideo,
    auth: &UrlAuth,
    url: &str,
    output: &Path,
    extra: &[&str],
) -> Command {
    let mut y = ytdlp.command();
    y.args(["--quiet", "--no-part", "--socket-timeout", "30", "-f", "bestaudio/best", "-o"]).arg(output).args(extra);
    if let Some(dir) = ffmpeg.ffmpeg_path().parent() {
        y.arg("--ffmpeg-location").arg(dir); // HLS 등에서 yt-dlp 가 쓰는 ffmpeg
    }
    match video.playlist_index {
        Some(i) => {
            y.args(["--playlist-items", &i.to_string()]);
        }
        None => {
            y.arg("--no-playlist");
        }
    }
    // 사용자 입력(URL)은 항상 `--` 뒤에 별도 인자로 → 옵션으로 해석될 수 없음
    y.args(auth.args()).args(video.request.args()).arg("--").arg(url);
    y
}

/// ffmpeg 명령: `input` 의 첫 오디오 트랙만 16 kHz mono PCM WAV 로
fn ffmpeg_command(ffmpeg: &FfmpegTools, input: &std::ffi::OsStr, out_wav: &Path) -> Command {
    let mut f = ffmpeg.ffmpeg_command();
    f.args(["-hide_banner", "-loglevel", "error", "-nostats", "-y", "-i"])
        .arg(input)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .args(["-progress", "pipe:1", "-f", "wav"])
        .arg(out_wav)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    f
}

#[allow(clippy::too_many_arguments)]
pub(super) fn pipe_extract(
    ytdlp: &YtDlp,
    ffmpeg: &FfmpegTools,
    video: &UrlVideo,
    auth: &UrlAuth,
    url: &str,
    out_wav: &Path,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(f32),
) -> Result<PipeOutcome, UrlError> {
    let mut ychild = ytdlp_command(ytdlp, ffmpeg, video, auth, url, Path::new("-"), &[])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| UrlError::Spawn { tool: "yt-dlp", source })?;
    let ystderr = ychild.stderr.take().map(|s| spawn_tail_reader(s, 16 * 1024));
    let Some(ystdout) = ychild.stdout.take() else {
        kill(&mut ychild);
        return Err(UrlError::Extract("영상 받기를 시작할 수 없습니다".to_owned()));
    };
    let mut f = ffmpeg_command(ffmpeg, std::ffi::OsStr::new("pipe:0"), out_wav);
    let mut fchild = match f.stdin(Stdio::from(ystdout)).spawn() {
        Ok(c) => c,
        Err(source) => {
            kill(&mut ychild);
            return Err(UrlError::Spawn { tool: "ffmpeg", source });
        }
    };
    let fstderr = fchild.stderr.take().map(|s| spawn_tail_reader(s, 8 * 1024));

    let total_us = video.duration.map(|d| d.as_micros() as f64).filter(|t| *t > 0.0);
    let ((fstatus, ystatus), cancelled) = watch_cancel(&[ychild.id(), fchild.id()], cancel, || {
        if let Some(stdout) = fchild.stdout.take() {
            read_ffmpeg_progress(stdout, total_us, &mut |p| emit(p));
        }
        (fchild.wait(), ychild.wait())
    });
    let ystderr = ystderr.and_then(|t| t.join().ok()).unwrap_or_default();
    let fstderr = fstderr.and_then(|t| t.join().ok()).unwrap_or_default();
    if cancelled {
        return Err(UrlError::Cancelled);
    }
    let (ystatus, fstatus) = (ystatus?, fstatus?);
    let empty = std::fs::metadata(out_wav).map(|m| m.len()).unwrap_or(0) <= 1024;
    if !ystatus.success() {
        let err = classify_error(&ystderr);
        // ffmpeg 가 먼저 포기하면 yt-dlp 는 파이프가 끊겨 일반 오류로 끝난다 → 파일로 재시도.
        // 로그인·DRM·미지원 등 분명한 yt-dlp 오류는 그대로 알린다.
        return match err {
            UrlError::Failed(_) if !fstatus.success() || empty => Ok(PipeOutcome::NeedsFile(err.to_string())),
            other => Err(other),
        };
    }
    if !fstatus.success() {
        return Ok(PipeOutcome::NeedsFile(last_line(&fstderr)));
    }
    if empty {
        return Ok(PipeOutcome::NeedsFile("파이프 입력에서 오디오를 읽지 못함".to_owned()));
    }
    Ok(PipeOutcome::Done)
}

#[allow(clippy::too_many_arguments)]
fn file_extract(
    ytdlp: &YtDlp,
    ffmpeg: &FfmpegTools,
    video: &UrlVideo,
    auth: &UrlAuth,
    url: &str,
    out_wav: &Path,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(f32),
) -> Result<(), UrlError> {
    // 받은 영상은 WAV 옆의 전용 임시 폴더에 (이름 충돌 없음). 함수가 끝나면(어떤 경우든) 폴더째 지워진다.
    // 고정 파일명을 주면 yt-dlp 가 재생목록 페이지에서 거부하므로 `media.<확장자>` 템플릿을 쓴다.
    let dir = out_wav.parent().unwrap_or_else(|| Path::new("."));
    let work = TempDir::new_in(dir, "url")?;

    // 1. 받기 (진행률: 받은 바이트 / 전체 바이트)
    let progress_args = [
        "--progress",
        "--newline",
        "--force-overwrites",
        "--progress-template",
        "download:MT %(progress.downloaded_bytes)s %(progress.total_bytes,progress.total_bytes_estimate)s",
    ];
    let mut ychild = ytdlp_command(ytdlp, ffmpeg, video, auth, url, &work.path().join("media.%(ext)s"), &progress_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| UrlError::Spawn { tool: "yt-dlp", source })?;
    let ystderr = ychild.stderr.take().map(|s| spawn_tail_reader(s, 16 * 1024));
    let (ystatus, cancelled) = watch_cancel(&[ychild.id()], cancel, || {
        if let Some(stdout) = ychild.stdout.take() {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let mut parts = line.trim().strip_prefix("MT ").unwrap_or("").split_whitespace();
                if let (Some(Ok(done)), Some(Ok(total))) =
                    (parts.next().map(str::parse::<f64>), parts.next().map(str::parse::<f64>))
                    && total > 0.0
                {
                    emit((done / total).clamp(0.0, 1.0) as f32 * DOWNLOAD_SHARE);
                }
            }
        }
        ychild.wait()
    });
    let ystderr = ystderr.and_then(|t| t.join().ok()).unwrap_or_default();
    if cancelled {
        return Err(UrlError::Cancelled);
    }
    if !ystatus?.success() {
        return Err(classify_error(&ystderr));
    }
    let media = std::fs::read_dir(work.path())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.is_file() && p.file_stem().is_some_and(|s| s == "media"))
        .ok_or_else(|| UrlError::Extract("받은 영상 파일을 찾을 수 없습니다".to_owned()))?;

    // 2. 받은 파일에서 오디오만
    let mut fchild = ffmpeg_command(ffmpeg, media.as_os_str(), out_wav)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|source| UrlError::Spawn { tool: "ffmpeg", source })?;
    let fstderr = fchild.stderr.take().map(|s| spawn_tail_reader(s, 8 * 1024));
    let total_us = video.duration.map(|d| d.as_micros() as f64).filter(|t| *t > 0.0);
    let (fstatus, cancelled) = watch_cancel(&[fchild.id()], cancel, || {
        if let Some(stdout) = fchild.stdout.take() {
            read_ffmpeg_progress(stdout, total_us, &mut |p| emit(DOWNLOAD_SHARE + p * (1.0 - DOWNLOAD_SHARE)));
        }
        fchild.wait()
    });
    let fstderr = fstderr.and_then(|t| t.join().ok()).unwrap_or_default();
    if cancelled {
        return Err(UrlError::Cancelled);
    }
    if !fstatus?.success() {
        return Err(UrlError::Extract(last_line(&fstderr)));
    }
    Ok(())
    // `work` 가 여기서 사라지며 받은 영상 파일을 지운다
}

/// FFmpeg 가 http(s) 미디어 주소를 직접 읽어 오디오만 WAV 로 만든다.
fn direct_extract(
    ffmpeg: &FfmpegTools,
    direct: &super::ytdlp::DirectMedia,
    duration: Option<Duration>,
    out_wav: &Path,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(f32),
) -> Result<(), UrlError> {
    let url = super::safety::validate_url(&direct.url)?;
    let referer = super::safety::validate_url(&direct.referer)?; // 공백·개행 불가 → 헤더 주입 방지
    tracing::info!(media = %redact_url(&url), "미디어 주소에서 직접 오디오 추출");
    let mut f = ffmpeg.ffmpeg_command();
    f.args(["-hide_banner", "-loglevel", "error", "-nostats", "-y"])
        // 네트워크 프로토콜만 허용 (file:, 하위 프로세스 등 차단)
        .args(["-protocol_whitelist", "http,https,tcp,tls,crypto"])
        .args(["-rw_timeout", "30000000", "-reconnect", "1", "-reconnect_on_network_error", "1", "-reconnect_delay_max", "10"])
        .arg("-headers")
        .arg(format!("Referer: {referer}\r\n"))
        .arg("-i")
        .arg(&url)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .args(["-progress", "pipe:1", "-f", "wav"])
        .arg(out_wav)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut fchild = f.spawn().map_err(|source| UrlError::Spawn { tool: "ffmpeg", source })?;
    let fstderr = fchild.stderr.take().map(|s| spawn_tail_reader(s, 8 * 1024));
    let total_us = duration.map(|d| d.as_micros() as f64).filter(|t| *t > 0.0);
    let (fstatus, cancelled) = watch_cancel(&[fchild.id()], cancel, || {
        if let Some(stdout) = fchild.stdout.take() {
            read_ffmpeg_progress(stdout, total_us, &mut |p| emit(p));
        }
        fchild.wait()
    });
    let fstderr = fstderr.and_then(|t| t.join().ok()).unwrap_or_default();
    if cancelled {
        return Err(UrlError::Cancelled);
    }
    if !fstatus?.success() {
        let line = last_line(&fstderr);
        if line.contains("403") || line.contains("401") {
            return Err(UrlError::AuthRequired(line));
        }
        return Err(UrlError::Extract(line));
    }
    Ok(())
}

/// `body` 를 실행하는 동안 취소 요청을 감시하다가, 요청되면 `pids` 프로세스를 모두 끝낸다.
/// 반환: (body 결과, 취소되었는지)
pub(super) fn watch_cancel<R>(pids: &[u32], cancel: &AtomicBool, body: impl FnOnce() -> R) -> (R, bool) {
    let done = AtomicBool::new(false);
    let cancelled = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    cancelled.store(true, Ordering::Relaxed);
                    pids.iter().for_each(|&pid| terminate(pid));
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let r = body();
        done.store(true, Ordering::Relaxed);
        r
    });
    (result, cancelled.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed))
}

/// ffmpeg `-progress pipe:1` 출력에서 진행률(0~1)을 읽는다. 전체 길이를 모르면 보고하지 않는다.
fn read_ffmpeg_progress(stdout: impl Read, total_us: Option<f64>, emit: &mut dyn FnMut(f32)) {
    let mut last = -1.0f32;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if let (Some(total), Some(v)) = (total_us, line.strip_prefix("out_time_us="))
            && let Ok(us) = v.trim().parse::<f64>()
        {
            let p = (us / total).clamp(0.0, 1.0) as f32;
            if p - last >= 0.01 {
                last = p;
                emit(p);
            }
        }
    }
}

fn last_line(stderr: &[u8]) -> String {
    let msg = String::from_utf8_lossy(stderr);
    let line = msg.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("알 수 없는 오류");
    super::safety::redact_text(line)
}

/// 다른 스레드에서 PID 로 프로세스를 종료한다 (Child 를 공유하지 않기 위해).
fn terminate(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

