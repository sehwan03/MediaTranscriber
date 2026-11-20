//! yt-dlp 자동 설치.
//!
//! - 출처: 공식 GitHub 릴리스 (github.com/yt-dlp/yt-dlp, Unlicense)
//! - 같은 릴리스의 `SHA2-256SUMS` 와 대조한 뒤에만 설치한다.
//! - 앱 데이터 폴더(`%LOCALAPPDATA%\MediaTranscriber\data\yt-dlp`)에만 설치한다.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::ytdlp::{UrlError, YtDlp};

const RELEASE_BASE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
const ASSET: &str = "yt-dlp.exe";
const CHECKSUMS: &str = "SHA2-256SUMS";
pub const APPROX_DOWNLOAD_MB: u32 = 23;

pub fn source_description() -> String {
    "yt-dlp 공식 릴리스 (GitHub · Unlicense)".to_owned()
}

fn find_checksum(list: &str, name: &str) -> Option<String> {
    list.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// 내려받아 검증·설치한 뒤 버전 문자열을 돌려준다. `on_progress(받은 바이트, 전체)`
pub fn install(dest_dir: &Path, cancel: &AtomicBool, mut on_progress: impl FnMut(u64, u64)) -> Result<(PathBuf, String), UrlError> {
    std::fs::create_dir_all(dest_dir)?;
    let http = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(60)) // 읽기 동작마다 적용
        .user_agent(concat!("MediaTranscriber/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| UrlError::Failed(e.to_string()))?;

    let sums = http
        .get(format!("{RELEASE_BASE}/{CHECKSUMS}"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .and_then(reqwest::blocking::Response::text)
        .map_err(|e| UrlError::Failed(format!("체크섬 목록을 받을 수 없습니다: {}", e.without_url())))?;
    let expected = find_checksum(&sums, ASSET).ok_or_else(|| UrlError::Failed("체크섬 목록에 yt-dlp.exe 가 없습니다".to_owned()))?;

    let part = dest_dir.join(format!("{ASSET}.part"));
    let result = download(&http, &part, &expected, cancel, &mut on_progress);
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
        result?;
    }
    let target = dest_dir.join(ASSET);
    std::fs::rename(&part, &target)?;

    let version = YtDlp::from_path(target.clone()).version()?;
    tracing::info!(dir = %dest_dir.display(), %version, "yt-dlp 설치 완료");
    Ok((target, version))
}

fn download(
    http: &reqwest::blocking::Client,
    path: &Path,
    expected: &str,
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<(), UrlError> {
    let mut resp = http
        .get(format!("{RELEASE_BASE}/{ASSET}"))
        .send()
        .map_err(|e| UrlError::Failed(format!("다운로드 실패: {}", e.without_url())))?;
    if !resp.status().is_success() {
        return Err(UrlError::Failed(format!("다운로드 실패: HTTP {}", resp.status().as_u16())));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut file = File::create(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let (mut done, mut last) = (0u64, 0u64);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(UrlError::Cancelled);
        }
        let n = match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(UrlError::Failed(format!("다운로드 중단: {e}"))),
        };
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
        done += n as u64;
        if done - last >= 512 * 1024 {
            last = done;
            on_progress(done, total);
        }
    }
    file.flush()?;
    drop(file);
    on_progress(done, total.max(done));
    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return Err(UrlError::Failed("무결성 검증 실패: SHA-256 이 일치하지 않습니다".to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_exe_checksum() {
        let list = "e51e69e89489f199530f13d32a4342da3ddb2cf34842f364603a066db05afbf3  yt-dlp\n\
                    f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d  yt-dlp.exe\n";
        assert_eq!(
            find_checksum(list, "yt-dlp.exe").as_deref(),
            Some("f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d")
        );
        assert!(find_checksum(list, "yt-dlp_x86.exe").is_none());
    }
}
