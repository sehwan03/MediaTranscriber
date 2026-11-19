//! GUI 와 분리된 미디어 검사 스레드.
//!
//! GUI 는 요청을 채널로 보내고, 매 프레임 `try_recv` 로 결과만 가져간다.
//! FFmpeg/ffprobe 프로세스 실행은 전부 이 스레드에서 일어난다.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use super::{FfmpegTools, MediaInfo};

pub enum MediaRequest {
    /// FFmpeg 를 (다시) 찾는다. 인자는 사용자가 지정한 폴더.
    Locate(Option<PathBuf>),
    Probe { id: u64, path: PathBuf },
}

pub enum MediaEvent {
    /// Ok(버전) / Err(사용자 표시용 메시지)
    Tools(Result<String, String>),
    Probed { id: u64, result: Result<MediaInfo, String> },
}

pub struct MediaWorker {
    tx: Sender<MediaRequest>,
    rx: Receiver<MediaEvent>,
}

impl MediaWorker {
    /// `notify` 는 결과가 나올 때마다 호출된다 (GUI 재그리기 요청용).
    pub fn spawn(
        ffmpeg_dir: Option<PathBuf>,
        notify: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let (req_tx, req_rx) = mpsc::channel::<MediaRequest>();
        let (evt_tx, evt_rx) = mpsc::channel::<MediaEvent>();

        std::thread::Builder::new()
            .name("media-worker".to_owned())
            .spawn(move || run(req_rx, evt_tx, notify))?;

        let worker = Self { tx: req_tx, rx: evt_rx };
        worker.send(MediaRequest::Locate(ffmpeg_dir));
        Ok(worker)
    }

    pub fn send(&self, request: MediaRequest) {
        if self.tx.send(request).is_err() {
            tracing::error!("미디어 작업 스레드가 종료되어 요청을 보낼 수 없음");
        }
    }

    pub fn try_recv(&self) -> Option<MediaEvent> {
        match self.rx.try_recv() {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => None,
        }
    }
}

/// 요청 채널이 닫히면(= MediaWorker drop) 루프가 끝나고 스레드가 종료된다.
fn run(rx: Receiver<MediaRequest>, tx: Sender<MediaEvent>, notify: impl Fn()) {
    let mut tools: Option<FfmpegTools> = None;

    for request in rx {
        let event = match request {
            MediaRequest::Locate(dir) => {
                let result = FfmpegTools::locate(dir.as_deref());
                let event = match &result {
                    Ok(t) => MediaEvent::Tools(Ok(t.version().to_owned())),
                    Err(e) => {
                        tracing::warn!(error = %e, "FFmpeg 탐색 실패");
                        MediaEvent::Tools(Err(e.to_string()))
                    }
                };
                tools = result.ok();
                event
            }
            MediaRequest::Probe { id, path } => {
                let result = match &tools {
                    Some(t) => t.probe(&path).map_err(|e| {
                        tracing::info!(file = %path.display(), error = %e, "미디어 검사 실패");
                        e.to_string()
                    }),
                    None => Err(super::MediaError::FfmpegNotFound.to_string()),
                };
                MediaEvent::Probed { id, result }
            }
        };
        if tx.send(event).is_err() {
            break; // GUI 종료
        }
        notify();
    }
    tracing::debug!("미디어 작업 스레드 종료");
}
