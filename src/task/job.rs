//! 작업·이벤트 타입과 공통 오류 변환.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::config::secret::ApiKey;
use crate::config::settings::CloudSettings;
use crate::config::{Language, WhisperModel};
use crate::engine_selector::BenchmarkResult;
use crate::media::MediaError;
use crate::output::OutputError;
use crate::speech::SpeechError;
use crate::speech::cloud::CloudError;
use crate::url::{UrlAuth, UrlVideo};

/// 작업에 사용할 엔진.
pub enum JobEngine {
    Local { model: WhisperModel, model_path: PathBuf },
    /// 사용자가 전송에 동의한 Cloud 작업. 키는 작업이 끝나면 메모리에서 지워진다.
    Cloud { settings: CloudSettings, api_key: ApiKey },
}

/// URL 영상 작업 정보 (선택한 영상 하나)
pub struct UrlJob {
    pub video: UrlVideo,
    pub auth: UrlAuth,
    pub ytdlp_dir: PathBuf,
    pub temp_dir: PathBuf,
}

pub struct TranscribeJob {
    pub id: u64,
    /// 파일 작업의 입력 경로 (URL 작업은 화면 표시용 이름)
    pub input: PathBuf,
    /// URL 작업이면 Some — 시작할 때 오디오를 임시 WAV 로 받아 같은 과정으로 처리한다.
    pub url: Option<UrlJob>,
    pub duration: Option<Duration>,
    pub output_dir: PathBuf,
    pub engine: JobEngine,
    pub language: Language,
    pub ffmpeg_dir: Option<PathBuf>,
    pub cancel: Arc<AtomicBool>,
}

/// 로컬 처리 성능 측정 요청.
pub struct BenchmarkJob {
    pub model: WhisperModel,
    pub model_path: PathBuf,
    /// 실제 음성 샘플로 쓸 파일 (앞 30초). 없으면 합성 신호 사용.
    pub sample: Option<PathBuf>,
    pub ffmpeg_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Downloading,
    LoadingModel,
    Decoding,
    Transcribing,
    Uploading,
    Saving,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Downloading => "영상 오디오 받는 중",
            Self::LoadingModel => "모델 로드 중",
            Self::Decoding => "오디오 추출 중",
            Self::Transcribing => "인식 중",
            Self::Uploading => "업로드·인식 중",
            Self::Saving => "저장 중",
        }
    }
}

pub enum TaskEvent {
    Progress { id: u64, stage: Stage, progress: f32 },
    Completed { id: u64, output: PathBuf, elapsed: Duration },
    Failed { id: u64, error: String },
    Cancelled { id: u64 },
    BenchmarkDone { model: WhisperModel, result: Result<BenchmarkResult, String> },
}

#[derive(Debug)]
pub(super) enum JobError {
    Cancelled,
    Failed(String),
}

impl From<MediaError> for JobError {
    fn from(e: MediaError) -> Self {
        match e {
            MediaError::Cancelled => Self::Cancelled,
            other => Self::Failed(other.to_string()),
        }
    }
}

impl From<SpeechError> for JobError {
    fn from(e: SpeechError) -> Self {
        match e {
            SpeechError::Cancelled => Self::Cancelled,
            other => Self::Failed(other.to_string()),
        }
    }
}

impl From<CloudError> for JobError {
    fn from(e: CloudError) -> Self {
        match e {
            CloudError::Cancelled => Self::Cancelled,
            other => Self::Failed(other.to_string()),
        }
    }
}

impl From<OutputError> for JobError {
    fn from(e: OutputError) -> Self {
        Self::Failed(e.to_string())
    }
}

/// 처리 결과를 GUI 이벤트로 바꾸고 로그를 남긴다. (파일 경로/오류만 기록, 인식 텍스트는 기록하지 않음)
pub(super) fn finish_event(job: &TranscribeJob, result: Result<PathBuf, JobError>, elapsed: Duration) -> TaskEvent {
    let id = job.id;
    match result {
        Ok(output) => {
            tracing::info!(id, elapsed_ms = elapsed.as_millis(), "변환 완료");
            TaskEvent::Completed { id, output, elapsed }
        }
        Err(JobError::Cancelled) => {
            tracing::info!(id, "변환 취소");
            TaskEvent::Cancelled { id }
        }
        Err(JobError::Failed(error)) => {
            // URL 작업은 표시 이름만 (주소·토큰을 로그에 남기지 않음)
            let file = match &job.url {
                Some(u) => crate::url::safety::redact_url(&u.video.source_url),
                None => job.input.display().to_string(),
            };
            tracing::warn!(id, %file, url = job.url.is_some(), %error, "변환 실패");
            TaskEvent::Failed { id, error }
        }
    }
}

/// 진행률 구간: 준비/추출 0~10%, 인식 10~98%, 저장 98~100%
pub(super) const DECODE_SPAN: f32 = 0.10;
pub(super) const TRANSCRIBE_END: f32 = 0.98;

pub(super) type Notify = Arc<dyn Fn() + Send + Sync>;
