//! 영상 파일 받기 (부가 기능, 설정에서 켜야 사용).
//!
//! - 최고 화질: `bv*+ba/b` (가장 좋은 영상 + 가장 좋은 오디오, 없으면 합쳐진 최고 형식).
//!   합칠 때는 가능하면 mp4, 코덱이 맞지 않으면 mkv 로 저장한다 (재인코딩 없음).
//! - 앱 임시 폴더 안의 전용 폴더에 받고, 끝까지 성공한 파일만 저장 폴더로 옮긴다.
//!   실패·취소 시 받던 조각은 폴더째 지워진다.
//! - 저장 폴더의 기존 파일은 덮어쓰지 않는다 (`제목 (1).mp4` …).
//! - 플레이어 설정에서 찾은 미디어(직접 주소)는 그 주소를 Referer 와 함께 받는다.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::AtomicBool;

use super::extract::watch_cancel;
use super::safety::{redact_url, safe_file_stem, validate_url};
use super::ytdlp::{UrlAuth, UrlError, UrlVideo, YtDlp, auth_kind, classify_error, spawn_tail_reader};
use crate::media::FfmpegTools;
use crate::utils::tempfile::TempDir;

/// 영상 받기 단계
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStage {
    Downloading,
    /// 영상·오디오를 하나로 합치는 중
    Merging,
}

/// 선택한 영상을 최고 화질로 `out_dir` 에 저장한다. 저장한 파일 경로를 돌려준다.
#[allow(clippy::too_many_arguments)]
pub fn download_video(
    ytdlp: &YtDlp,
    ffmpeg: &FfmpegTools,
    video: &UrlVideo,
    auth: &UrlAuth,
    out_dir: &Path,
    temp_root: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(DownloadStage, f32),
) -> Result<PathBuf, UrlError> {
    std::fs::create_dir_all(out_dir)?;
    let work = TempDir::new_in(temp_root, "url")?;

    // 직접 주소면 그 주소를, 아니면 탐색했던 주소를 yt-dlp 에 넘긴다
    let (url, referer, index) = match &video.direct {
        Some(d) => (validate_url(&d.url)?, Some(d.referer.clone()), None),
        None => (validate_url(&video.source_url)?, video.request.referer.clone(), video.playlist_index),
    };
    tracing::info!(url = %redact_url(&url), index = ?index, auth = auth_kind(auth), "영상 받기 시작");

    let mut y = ytdlp.command();
    y.args(["--quiet", "--progress", "--newline", "--no-part", "--no-simulate", "--socket-timeout", "30"])
        .args(["-f", "bv*+ba/b", "--merge-output-format", "mp4/mkv"])
        .args(["--progress-template", "download:MT %(progress.downloaded_bytes)s %(progress.total_bytes,progress.total_bytes_estimate)s"])
        // 받을 형식 개수(영상+오디오면 2)와 최종 파일 경로를 알려 달라고 한다
        .args(["--print", "before_dl:MTFMT %(format_id)s", "--print", "after_move:MTFILE %(filepath)s"])
        .arg("-o")
        .arg(work.path().join("video.%(ext)s"));
    if let Some(dir) = ffmpeg.ffmpeg_path().parent() {
        y.arg("--ffmpeg-location").arg(dir); // 영상·오디오 합치기에 필요
    }
    match index {
        Some(i) => {
            y.args(["--playlist-items", &i.to_string()]);
        }
        None => {
            y.arg("--no-playlist");
        }
    }
    let mut request = video.request.clone();
    request.referer = referer;
    y.args(auth.args()).args(request.args()).arg("--").arg(&url);

    let mut child = y
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| UrlError::Spawn { tool: "yt-dlp", source })?;
    let stderr = child.stderr.take().map(|s| spawn_tail_reader(s, 16 * 1024));
    let mut final_path: Option<PathBuf> = None;
    let (status, cancelled) = watch_cancel(&[child.id()], cancel, || {
        if let Some(stdout) = child.stdout.take() {
            let mut parts = 1usize;
            let mut part = 0usize;
            let mut last = 0.0f64;
            let mut best = 0.0f32;
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let line = line.trim();
                if let Some(fmt) = line.strip_prefix("MTFMT ") {
                    parts = fmt.split('+').count().max(1);
                } else if let Some(p) = line.strip_prefix("MTFILE ") {
                    final_path = Some(PathBuf::from(p));
                } else if let Some(rest) = line.strip_prefix("MT ") {
                    let mut it = rest.split_whitespace().map(str::parse::<f64>);
                    if let (Some(Ok(done)), Some(Ok(total))) = (it.next(), it.next())
                        && total > 0.0
                    {
                        let frac = (done / total).clamp(0.0, 1.0);
                        // 두 번째 형식(오디오)이 시작되면 진행률이 다시 0 부터 → 다음 부분
                        if frac + 0.5 < last && part + 1 < parts {
                            part += 1;
                        }
                        last = frac;
                        let overall = ((part as f64 + frac) / parts as f64) as f32 * 0.97;
                        if overall > best {
                            best = overall;
                            on_progress(DownloadStage::Downloading, overall);
                        }
                        if frac >= 1.0 && part + 1 == parts && parts > 1 {
                            on_progress(DownloadStage::Merging, 0.97);
                        }
                    }
                }
            }
        }
        child.wait()
    });
    let stderr = stderr.and_then(|t| t.join().ok()).unwrap_or_default();
    if cancelled {
        return Err(UrlError::Cancelled);
    }
    if !status?.success() {
        return Err(classify_error(&stderr));
    }

    // 받은 파일 (출력 경로를 못 받았으면 작업 폴더에서 찾는다)
    let file = final_path
        .filter(|p| p.is_file() && p.starts_with(work.path()))
        .or_else(|| {
            std::fs::read_dir(work.path())
                .ok()?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .find(|p| p.is_file() && p.file_stem().is_some_and(|s| s == "video"))
        })
        .ok_or_else(|| UrlError::Failed("받은 영상 파일을 찾을 수 없습니다".to_owned()))?;
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    let dest = unique_path(out_dir, &safe_file_stem(&video.title), &ext);
    move_file(&file, &dest)?;
    on_progress(DownloadStage::Downloading, 1.0);
    tracing::info!(ext = %ext, "영상 받기 완료");
    Ok(dest)
}

/// `dir/stem.ext` 가 있으면 `stem (1).ext`, `stem (2).ext` …
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (1..10_000)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// 같은 드라이브면 이름 바꾸기, 아니면 복사 후 삭제
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).inspect_err(|_| {
        let _ = std::fs::remove_file(to);
    })?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names_do_not_overwrite() {
        let dir = std::env::temp_dir().join(format!("media_transcriber_dl_names_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert_eq!(unique_path(&dir, "강의", "mp4"), dir.join("강의.mp4"));
        std::fs::write(dir.join("강의.mp4"), b"x").expect("write");
        assert_eq!(unique_path(&dir, "강의", "mp4"), dir.join("강의 (1).mp4"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
