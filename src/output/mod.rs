//! 결과 저장. 현재 TXT, 이후 SRT/VTT/JSON 을 같은 `Transcript` 로부터 추가한다.

pub mod txt;

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("인식된 텍스트가 없어 저장하지 않았습니다 (음성이 없거나 너무 작을 수 있습니다)")]
    EmptyResult,

    #[error("결과 파일 저장 실패 ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("같은 이름의 결과 파일이 너무 많습니다: {0}")]
    TooManyDuplicates(String),
}

impl OutputError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io { path: path.to_path_buf(), source }
    }
}
