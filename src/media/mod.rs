//! 미디어 처리: FFmpeg 탐색, 미디어 검사(ffprobe), 오디오 디코딩.
//!
//! 모든 함수는 블로킹이며 GUI 스레드에서 호출하면 안 된다.
//! GUI 는 [`worker::MediaWorker`] 를 통해 백그라운드 스레드에서 사용한다.

pub mod audio;
pub mod ffmpeg;
pub mod installer;
pub mod worker;

use std::path::PathBuf;
use std::time::Duration;

pub use ffmpeg::FfmpegTools;

/// Whisper 입력 샘플레이트 (16 kHz, mono, f32).
pub const WHISPER_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("FFmpeg 를 찾을 수 없습니다. 설정에서 FFmpeg 폴더를 지정하거나 FFmpeg 를 설치하세요.")]
    FfmpegNotFound,

    #[error("{tool} 실행 실패: {source}")]
    Spawn {
        tool: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("파일이 없습니다: {0}")]
    FileNotFound(PathBuf),

    #[error("빈 파일입니다 (0 바이트)")]
    EmptyFile,

    #[error("미디어 파일로 읽을 수 없습니다: {0}")]
    InvalidMedia(String),

    #[error("오디오 트랙이 없습니다")]
    NoAudioStream,

    #[error("오디오 디코딩 실패: {0}")]
    DecodeFailed(String),

    #[error("디코딩 결과 오디오가 비어 있습니다")]
    EmptyAudio,

    #[error("작업이 취소되었습니다")]
    Cancelled,

    #[error("입출력 오류: {0}")]
    Io(#[from] std::io::Error),
}

/// 오디오 스트림 정보.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioStreamInfo {
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}

/// ffprobe 로 확인한 미디어 정보.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub format_name: String,
    pub duration: Option<Duration>,
    pub has_video: bool,
    pub audio: AudioStreamInfo,
}

/// `1:02:03` / `02:03` 형식으로 표시.
pub fn format_duration(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn duration_format() {
        assert_eq!(format_duration(Duration::from_secs(5)), "00:05");
        assert_eq!(format_duration(Duration::from_secs(125)), "02:05");
        assert_eq!(format_duration(Duration::from_secs(3723)), "1:02:03");
    }
}
