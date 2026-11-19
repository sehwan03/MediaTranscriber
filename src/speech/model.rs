//! Whisper 모델 파일 관리: 설치 확인, 다운로드, SHA-256 무결성 검증, 삭제.
//!
//! 모든 함수는 블로킹이다. GUI 에서는 [`super::model_worker`] 를 통해 백그라운드에서 호출한다.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::config::WhisperModel;

/// 공식 whisper.cpp ggml 모델 저장소 (Hugging Face).
const MODEL_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// 모델 파일 정보. 크기/해시는 Hugging Face API(LFS oid)에서 확인한 값이다.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub file_name: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
}

pub fn spec(model: WhisperModel) -> ModelSpec {
    match model {
        WhisperModel::Tiny => ModelSpec {
            file_name: "ggml-tiny.bin",
            size_bytes: 77_691_713,
            sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        },
        WhisperModel::Base => ModelSpec {
            file_name: "ggml-base.bin",
            size_bytes: 147_951_465,
            sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        },
        WhisperModel::Small => ModelSpec {
            file_name: "ggml-small.bin",
            size_bytes: 487_601_967,
            sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        },
        WhisperModel::Medium => ModelSpec {
            file_name: "ggml-medium.bin",
            size_bytes: 1_533_763_059,
            sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        },
    }
}

pub fn model_path(models_dir: &Path, model: WhisperModel) -> PathBuf {
    models_dir.join(spec(model).file_name)
}

/// 파일 크기만으로 빠르게 확인한 설치 상태 (해시 계산 없음).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFileState {
    Missing,
    Installed,
    /// 크기가 맞지 않음 (다운로드 중단, 손상 등)
    Corrupted,
}

pub fn quick_check(models_dir: &Path, model: WhisperModel) -> ModelFileState {
    match std::fs::metadata(model_path(models_dir, model)) {
        Ok(meta) if meta.is_file() && meta.len() == spec(model).size_bytes => ModelFileState::Installed,
        Ok(_) => ModelFileState::Corrupted,
        Err(_) => ModelFileState::Missing,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("네트워크 오류: {0}")]
    Network(String),

    #[error("서버 응답 오류: HTTP {0}")]
    HttpStatus(u16),

    #[error("다운로드 크기가 맞지 않습니다 (받은 크기 {got} / 예상 {expected} 바이트)")]
    SizeMismatch { got: u64, expected: u64 },

    #[error("무결성 검증 실패: 해시가 일치하지 않습니다. 파일이 손상되었을 수 있습니다.")]
    HashMismatch,

    #[error("디스크 공간이 부족합니다")]
    DiskFull,

    #[error("작업이 취소되었습니다")]
    Cancelled,

    #[error("파일 오류 ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> ModelError + '_ {
    move |source| {
        // Windows ERROR_DISK_FULL(112) / ERROR_HANDLE_DISK_FULL(39)
        if matches!(source.raw_os_error(), Some(112 | 39)) {
            return ModelError::DiskFull;
        }
        ModelError::Io { path: path.to_path_buf(), source }
    }
}

/// 모델을 내려받아 `.part` 파일에 쓰고, 크기·SHA-256 검증 후 최종 파일로 교체한다.
/// 실패/취소 시 `.part` 파일을 정리한다.
///
/// `on_progress(받은 바이트, 전체 바이트)`
pub fn download(
    models_dir: &Path,
    model: WhisperModel,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, ModelError> {
    let spec = spec(model);
    std::fs::create_dir_all(models_dir).map_err(io_err(models_dir))?;
    let final_path = models_dir.join(spec.file_name);
    let part_path = models_dir.join(format!("{}.part", spec.file_name));

    let result = download_to(&part_path, spec, cancel, &mut on_progress)
        .and_then(|()| std::fs::rename(&part_path, &final_path).map_err(io_err(&final_path)));

    if result.is_err() {
        // 정리 실패는 다음 다운로드 때 덮어쓰므로 치명적이지 않다.
        if let Err(e) = std::fs::remove_file(&part_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(file = %part_path.display(), error = %e, "임시 다운로드 파일 삭제 실패");
        }
    }
    result.map(|()| final_path)
}

fn download_to(
    part_path: &Path,
    spec: ModelSpec,
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<(), ModelError> {
    let url = format!("{MODEL_BASE_URL}/{}", spec.file_name);
    tracing::info!(%url, "모델 다운로드 시작");

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        // blocking 클라이언트의 timeout 은 전체 시간이 아니라 연결/읽기/쓰기 각 동작 단위로 적용된다.
        // (응답 본문 read() 호출마다 적용) → 대용량 모델도 끊기지 않고, 60초 이상 멈추면 실패 처리.
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("MediaTranscriber/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ModelError::Network(e.to_string()))?;

    let mut response = client.get(&url).send().map_err(|e| ModelError::Network(e.to_string()))?;
    if !response.status().is_success() {
        return Err(ModelError::HttpStatus(response.status().as_u16()));
    }

    let mut file = File::create(part_path).map_err(io_err(part_path))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut received: u64 = 0;
    let mut last_report = 0u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ModelError::Cancelled);
        }
        let n = match response.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ModelError::Network(e.to_string())),
        };
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(io_err(part_path))?;
        received += n as u64;
        if received > spec.size_bytes {
            return Err(ModelError::SizeMismatch { got: received, expected: spec.size_bytes });
        }
        // 진행률 이벤트 과다 방지 (약 1MB 단위)
        if received - last_report >= 1024 * 1024 || received == spec.size_bytes {
            last_report = received;
            on_progress(received, spec.size_bytes);
        }
    }
    file.flush().map_err(io_err(part_path))?;
    file.sync_all().map_err(io_err(part_path))?;
    drop(file);

    if received != spec.size_bytes {
        return Err(ModelError::SizeMismatch { got: received, expected: spec.size_bytes });
    }
    if to_hex(&hasher.finalize()) != spec.sha256 {
        return Err(ModelError::HashMismatch);
    }
    tracing::info!(file = spec.file_name, "모델 다운로드 및 검증 완료");
    Ok(())
}

/// 설치된 모델 파일의 SHA-256 을 계산해 공식 값과 비교한다.
pub fn verify(
    models_dir: &Path,
    model: WhisperModel,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(), ModelError> {
    let spec = spec(model);
    let path = models_dir.join(spec.file_name);
    let file = File::open(&path).map_err(io_err(&path))?;
    let total = file.metadata().map_err(io_err(&path))?.len();
    if total != spec.size_bytes {
        return Err(ModelError::SizeMismatch { got: total, expected: spec.size_bytes });
    }
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ModelError::Cancelled);
        }
        let n = reader.read(&mut buf).map_err(io_err(&path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        on_progress(done, total);
    }
    if to_hex(&hasher.finalize()) == spec.sha256 { Ok(()) } else { Err(ModelError::HashMismatch) }
}

pub fn delete(models_dir: &Path, model: WhisperModel) -> Result<(), ModelError> {
    let path = model_path(models_dir, model);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(&path)(e)),
    }
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_sha256() {
        let digest = Sha256::digest(b"abc");
        assert_eq!(
            to_hex(&digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn quick_check_detects_missing_and_wrong_size() {
        let dir = std::env::temp_dir().join(format!("media_transcriber_model_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert_eq!(quick_check(&dir, WhisperModel::Tiny), ModelFileState::Missing);
        std::fs::write(model_path(&dir, WhisperModel::Tiny), b"broken").expect("write");
        assert_eq!(quick_check(&dir, WhisperModel::Tiny), ModelFileState::Corrupted);
        assert!(matches!(
            verify(&dir, WhisperModel::Tiny, &AtomicBool::new(false), |_, _| {}),
            Err(ModelError::SizeMismatch { .. })
        ));
        delete(&dir, WhisperModel::Tiny).expect("delete");
        assert_eq!(quick_check(&dir, WhisperModel::Tiny), ModelFileState::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
