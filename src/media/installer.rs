//! FFmpeg 자동 설치 (LGPL 빌드).
//!
//! - 출처: FFmpeg 공식 사이트(ffmpeg.org/download.html)가 안내하는 Windows 빌드인 BtbN/FFmpeg-Builds
//! - LGPL 구성(GPL 라이브러리 미포함)만 사용한다.
//! - 같은 릴리스의 `checksums.sha256` 과 SHA-256 을 대조한 뒤에만 압축을 푼다.
//! - ffmpeg.exe / ffprobe.exe / LICENSE.txt 만 앱 데이터 폴더에 설치한다.
//! - 실패·취소 시 내려받던 파일을 정리한다.
//!
//! "latest" 릴리스는 매일 다시 빌드되므로 해시를 코드에 고정하지 않고 릴리스의 체크섬 파일을 사용한다.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::FfmpegTools;

const RELEASE_BASE: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest";
const ASSET: &str = "ffmpeg-n9.0-latest-win64-lgpl-9.0.zip";
const CHECKSUMS: &str = "checksums.sha256";
/// 설치할 파일 (zip 안의 파일명 → 설치 파일명)
const WANTED: [&str; 3] = ["ffmpeg.exe", "ffprobe.exe", "LICENSE.txt"];

/// 화면 안내용 대략적인 다운로드 크기
pub const APPROX_DOWNLOAD_MB: u32 = 170;

pub fn source_description() -> String {
    format!("BtbN/FFmpeg-Builds (GitHub) · {ASSET} · LGPL")
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("네트워크 오류: {0}. 인터넷 연결을 확인하세요.")]
    Network(String),
    #[error("서버 응답 오류: HTTP {0}")]
    HttpStatus(u16),
    #[error("체크섬 목록에서 {0} 을(를) 찾을 수 없습니다")]
    ChecksumMissing(String),
    #[error("무결성 검증 실패: 내려받은 파일의 SHA-256 이 일치하지 않습니다")]
    HashMismatch,
    #[error("압축 파일 오류: {0}")]
    Archive(String),
    #[error("설치된 FFmpeg 를 실행할 수 없습니다: {0}")]
    Verify(String),
    #[error("설치된 FFmpeg 에 MP3 인코더(libmp3lame)가 없어 Cloud 업로드를 할 수 없습니다")]
    MissingEncoder,
    #[error("파일 오류 ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("작업이 취소되었습니다")]
    Cancelled,
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> InstallError + '_ {
    move |source| InstallError::Io { path: path.to_path_buf(), source }
}

fn client() -> Result<reqwest::blocking::Client, InstallError> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        // 각 읽기 동작마다 적용 (전체 시간 제한 아님)
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("MediaTranscriber/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| InstallError::Network(e.to_string()))
}

/// `sha256  파일명` 형식의 체크섬 목록에서 해당 파일의 해시를 찾는다.
fn find_checksum(list: &str, name: &str) -> Option<String> {
    list.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// FFmpeg 를 내려받아 `dest_dir` 에 설치하고, 실행 확인까지 마친 뒤 FfmpegTools 를 돌려준다.
/// `on_progress(받은 바이트, 전체 바이트)` — 전체를 모르면 0.
pub fn install(
    dest_dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<FfmpegTools, InstallError> {
    std::fs::create_dir_all(dest_dir).map_err(io_err(dest_dir))?;
    let http = client()?;

    // 1. 체크섬 목록
    let checksums = http
        .get(format!("{RELEASE_BASE}/{CHECKSUMS}"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .and_then(reqwest::blocking::Response::text)
        .map_err(|e| match e.status() {
            Some(s) => InstallError::HttpStatus(s.as_u16()),
            None => InstallError::Network(e.to_string()),
        })?;
    let expected = find_checksum(&checksums, ASSET).ok_or_else(|| InstallError::ChecksumMissing(ASSET.to_owned()))?;

    // 2. 다운로드 (해시를 계산하면서 저장)
    let zip_path = dest_dir.join("ffmpeg-download.zip.part");
    let result = download(&http, &zip_path, &expected, cancel, &mut on_progress)
        .and_then(|()| extract(&zip_path, dest_dir, cancel));
    if let Err(e) = std::fs::remove_file(&zip_path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(error = %e, "FFmpeg 임시 다운로드 파일 삭제 실패");
    }
    result?;

    // 3. 실행 확인 + Cloud 업로드에 필요한 인코더 확인
    let tools = FfmpegTools::locate(Some(dest_dir)).map_err(|e| InstallError::Verify(e.to_string()))?;
    if !tools.has_encoder("libmp3lame") {
        return Err(InstallError::MissingEncoder);
    }
    tracing::info!(dir = %dest_dir.display(), version = tools.version(), "FFmpeg 설치 완료");
    Ok(tools)
}

fn download(
    http: &reqwest::blocking::Client,
    path: &Path,
    expected: &str,
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<(), InstallError> {
    let url = format!("{RELEASE_BASE}/{ASSET}");
    tracing::info!(%url, "FFmpeg 다운로드 시작");
    let mut response = http.get(&url).send().map_err(|e| InstallError::Network(e.to_string()))?;
    if !response.status().is_success() {
        return Err(InstallError::HttpStatus(response.status().as_u16()));
    }
    let total = response.content_length().unwrap_or(0);
    let mut file = File::create(path).map_err(io_err(path))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let (mut done, mut last) = (0u64, 0u64);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        let n = match response.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(InstallError::Network(e.to_string())),
        };
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(io_err(path))?;
        done += n as u64;
        if done - last >= 1024 * 1024 {
            last = done;
            on_progress(done, total);
        }
    }
    file.flush().map_err(io_err(path))?;
    drop(file);
    on_progress(done, total.max(done));

    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return Err(InstallError::HashMismatch);
    }
    Ok(())
}

/// zip 에서 필요한 파일만 꺼내 임시 이름으로 쓴 뒤 교체한다.
fn extract(zip_path: &Path, dest_dir: &Path, cancel: &AtomicBool) -> Result<(), InstallError> {
    let file = File::open(zip_path).map_err(io_err(zip_path))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| InstallError::Archive(e.to_string()))?;
    let mut found = Vec::new();
    for i in 0..archive.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        let mut entry = archive.by_index(i).map_err(|e| InstallError::Archive(e.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        // enclosed_name: 경로 조작(../)이 있는 항목은 거부된다
        let Some(name) = entry.enclosed_name() else { continue };
        let Some(file_name) = name.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        if !WANTED.contains(&file_name.as_str()) || found.contains(&file_name) {
            continue;
        }
        let tmp = dest_dir.join(format!("{file_name}.tmp"));
        let mut out = File::create(&tmp).map_err(io_err(&tmp))?;
        std::io::copy(&mut entry, &mut out).map_err(io_err(&tmp))?;
        out.flush().map_err(io_err(&tmp))?;
        drop(out);
        let target = dest_dir.join(&file_name);
        std::fs::rename(&tmp, &target).map_err(io_err(&target))?;
        found.push(file_name);
    }
    for required in ["ffmpeg.exe", "ffprobe.exe"] {
        if !found.iter().any(|f| f == required) {
            return Err(InstallError::Archive(format!("압축 파일에 {required} 가 없습니다")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_checksum_list() {
        let list = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  ffmpeg-master-latest-win64-gpl.zip
ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789 *ffmpeg-n9.0-latest-win64-lgpl-9.0.zip
nothex  bad.zip";
        assert_eq!(
            find_checksum(list, ASSET).as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
        );
        assert!(find_checksum(list, "bad.zip").is_none());
        assert!(find_checksum(list, "missing.zip").is_none());
    }

    /// 실제 다운로드·설치 (약 170 MB). `cargo test ffmpeg_real_install -- --ignored --nocapture`
    #[test]
    #[ignore = "네트워크로 약 170MB 를 내려받는다"]
    fn ffmpeg_real_install() {
        let dir = std::env::temp_dir().join(format!("media_transcriber_ffmpeg_install_{}", std::process::id()));
        let mut last = 0;
        let tools = install(&dir, &AtomicBool::new(false), |done, total| {
            let pct = (done * 100).checked_div(total).unwrap_or(0);
            if pct >= last + 25 {
                last = pct;
                eprintln!("FFmpeg 다운로드 {pct}%");
            }
        })
        .expect("install");
        eprintln!("설치: {} (버전 {})", dir.display(), tools.version());
        assert!(dir.join("LICENSE.txt").is_file());
        assert!(!dir.join("ffmpeg-download.zip.part").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
