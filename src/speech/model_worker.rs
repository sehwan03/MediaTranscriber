//! 모델 다운로드/검증을 GUI 와 분리된 스레드에서 실행한다.
//! 작업마다 스레드를 하나 띄우고 결과는 공용 채널로 받는다.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use super::model;
use crate::config::WhisperModel;

pub enum ModelEvent {
    Progress { model: WhisperModel, done: u64, total: u64 },
    DownloadFinished { model: WhisperModel, result: Result<(), String> },
    VerifyFinished { model: WhisperModel, result: Result<(), String> },
}

pub struct ModelWorker {
    tx: Sender<ModelEvent>,
    rx: Receiver<ModelEvent>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl ModelWorker {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, notify: Arc::new(notify) }
    }

    pub fn try_recv(&self) -> Option<ModelEvent> {
        self.rx.try_recv().ok()
    }

    /// 다운로드 시작. 반환된 플래그를 true 로 바꾸면 취소된다.
    pub fn start_download(&self, models_dir: PathBuf, model: WhisperModel) -> Arc<AtomicBool> {
        self.spawn(model, move |cancel, progress| {
            let result = model::download(&models_dir, model, cancel, progress).map(|_| ());
            ModelEvent::DownloadFinished { model, result: result.map_err(|e| e.to_string()) }
        })
    }

    pub fn start_verify(&self, models_dir: PathBuf, model: WhisperModel) -> Arc<AtomicBool> {
        self.spawn(model, move |cancel, progress| {
            let result = model::verify(&models_dir, model, cancel, progress);
            ModelEvent::VerifyFinished { model, result: result.map_err(|e| e.to_string()) }
        })
    }

    fn spawn(
        &self,
        model: WhisperModel,
        job: impl FnOnce(&AtomicBool, &mut dyn FnMut(u64, u64)) -> ModelEvent + Send + 'static,
    ) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let tx = self.tx.clone();
        let notify = Arc::clone(&self.notify);

        let spawned = std::thread::Builder::new().name(format!("model-{}", model.label())).spawn(move || {
            let progress_tx = tx.clone();
            let progress_notify = Arc::clone(&notify);
            let mut progress = move |done: u64, total: u64| {
                let _ = progress_tx.send(ModelEvent::Progress { model, done, total });
                progress_notify();
            };
            let event = job(&thread_cancel, &mut progress);
            let _ = tx.send(event);
            notify();
        });

        if let Err(err) = spawned {
            tracing::error!(error = %err, "모델 작업 스레드 생성 실패");
            let _ = self.tx.send(ModelEvent::DownloadFinished {
                model,
                result: Err(format!("작업 스레드 생성 실패: {err}")),
            });
        }
        cancel
    }
}
