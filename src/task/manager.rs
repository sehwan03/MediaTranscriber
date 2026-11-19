//! 작업 관리자: GUI 가 사용하는 단일 창구.
//!
//! - Local 작업 → Local 작업 스레드 (동시 1개, 모델 재사용)
//! - Cloud 작업 → Cloud 대기열 (설정된 동시 요청 수만큼 병렬)
//! - 결과는 하나의 이벤트 채널로 모은다. GUI 는 매 프레임 `try_recv` 로 가져가기만 한다.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use super::job::{BenchmarkJob, JobEngine, Notify, TaskEvent, TranscribeJob};
use super::queue::{self, CloudMsg};
use super::worker::{self, LocalRequest};

pub struct TaskManager {
    local_tx: Sender<LocalRequest>,
    cloud_tx: Sender<CloudMsg>,
    events: Receiver<TaskEvent>,
}

const CLOSED: &str = "작업 스레드가 종료되었습니다";

impl TaskManager {
    pub fn spawn(notify: impl Fn() + Send + Sync + 'static) -> std::io::Result<Self> {
        let notify: Notify = Arc::new(notify);
        let (evt_tx, events) = mpsc::channel::<TaskEvent>();

        let (local_tx, local_rx) = mpsc::channel::<LocalRequest>();
        let (tx, n) = (evt_tx.clone(), Arc::clone(&notify));
        std::thread::Builder::new()
            .name("local-worker".to_owned())
            .spawn(move || worker::run(local_rx, tx, n))?;

        let (cloud_tx, cloud_rx) = mpsc::channel::<CloudMsg>();
        let self_tx = cloud_tx.clone();
        std::thread::Builder::new()
            .name("cloud-dispatcher".to_owned())
            .spawn(move || queue::run(cloud_rx, self_tx, evt_tx, notify))?;

        Ok(Self { local_tx, cloud_tx, events })
    }

    pub fn submit(&self, job: TranscribeJob) -> Result<(), String> {
        match job.engine {
            JobEngine::Local { .. } => self.local_tx.send(LocalRequest::Transcribe(Box::new(job))).map_err(|_| CLOSED.to_owned()),
            JobEngine::Cloud { .. } => self.cloud_tx.send(CloudMsg::Job(Box::new(job))).map_err(|_| CLOSED.to_owned()),
        }
    }

    /// Local 대기열 뒤에 성능 측정을 넣는다 (변환과 겹치지 않고 로드된 모델을 재사용).
    pub fn submit_benchmark(&self, job: BenchmarkJob) -> Result<(), String> {
        self.local_tx.send(LocalRequest::Benchmark(job)).map_err(|_| CLOSED.to_owned())
    }

    pub fn try_recv(&self) -> Option<TaskEvent> {
        self.events.try_recv().ok()
    }
}
