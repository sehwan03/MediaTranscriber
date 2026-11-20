//! 작업 입력 준비: 파일은 그대로, URL 영상은 오디오를 임시 WAV 로 추출해 넘긴다.
//!
//! 임시 WAV 는 [`TempFile`] 가드가 들고 있어 작업이 성공·실패·취소 어떻게 끝나도 지워진다.
//! 그 뒤의 처리(Local / Cloud 인식, TXT 저장)는 파일 작업과 완전히 같다.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::job::{JobError, Stage, TranscribeJob};
use crate::media::FfmpegTools;
use crate::url::{UrlError, YtDlp, extract::extract_audio};
use crate::utils::tempfile::TempFile;

/// URL 작업에서 오디오 받기가 차지하는 진행률 구간 (나머지는 기존 처리)
const URL_SPAN: f32 = 0.4;

impl From<UrlError> for JobError {
    fn from(e: UrlError) -> Self {
        match e {
            UrlError::Cancelled => Self::Cancelled,
            other => Self::Failed(other.to_string()),
        }
    }
}

pub(super) struct PreparedInput {
    pub path: PathBuf,
    pub duration: Option<Duration>,
    /// 결과 TXT 이름 (파일: 원본 이름, URL: 영상 제목)
    pub output_stem: String,
    /// URL 작업의 임시 오디오. 이 값이 사라지면 파일이 삭제된다.
    _temp: Option<TempFile>,
}

impl PreparedInput {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// 작업의 입력을 준비한다. `send` 는 원래 진행률 함수 (URL 은 0~40% 를 오디오 받기에 쓴다).
pub(super) fn prepare(job: &TranscribeJob, tools: &FfmpegTools, send: &dyn Fn(Stage, f32)) -> Result<PreparedInput, JobError> {
    let Some(url) = &job.url else {
        let stem = job
            .input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "transcript".to_owned());
        return Ok(PreparedInput { path: job.input.clone(), duration: job.duration, output_stem: stem, _temp: None });
    };

    let ytdlp = YtDlp::locate(Some(&url.ytdlp_dir))?;
    let temp = TempFile::new_in(&url.temp_dir, "url", "wav").map_err(|e| JobError::Failed(format!("임시 파일을 만들 수 없습니다: {e}")))?;
    send(Stage::Downloading, 0.0);
    extract_audio(&ytdlp, tools, &url.video, &url.auth, temp.path(), &job.cancel, |p| {
        send(Stage::Downloading, p * URL_SPAN);
    })?;
    // 실제 오디오 길이 (메타데이터가 없거나 부정확할 수 있으므로 WAV 를 직접 확인)
    let duration = tools.probe(temp.path()).ok().and_then(|i| i.duration).or(url.video.duration);
    Ok(PreparedInput {
        path: temp.path().to_path_buf(),
        duration,
        output_stem: crate::url::safety::safe_file_stem(&url.video.title),
        _temp: Some(temp),
    })
}

/// URL 작업에서 기존 처리 단계의 진행률(0~1)을 전체 진행률로 바꾼다 (앞 40% 는 오디오 받기).
pub(super) fn remap_url(p: f32) -> f32 {
    URL_SPAN + (1.0 - URL_SPAN) * p
}
