use std::path::PathBuf;

/// 애플리케이션 공통 오류 타입.
/// 이후 단계(FFmpeg, Whisper, Cloud)에서 변형을 추가한다.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("파일 입출력 오류 ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("설정 파일 형식 오류 ({path}): {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl AppError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}
