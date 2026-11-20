//! Cloud 작업 대기열: 설정된 동시 요청 수만큼만 병렬로 처리한다.
//!
//! 디스패처 스레드가 대기열(VecDeque)을 들고 있고, 슬롯이 비면 작업 스레드를 하나 띄운다.
//! 작업 스레드는 끝나면 `Done` 을 보내 슬롯을 반납한다. 대기 중 취소된 작업은 실행하지 않는다.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use super::job::{JobEngine, JobError, Notify, Stage, TRANSCRIBE_END, TaskEvent, TranscribeJob, finish_event};
use crate::config::CloudProvider;
use crate::media::FfmpegTools;
use crate::output::txt;
use crate::speech::cloud::CloudClient;

/// 동시 요청 수 상한 (설정값이 이보다 커도 제한).
pub const MAX_CLOUD_CONCURRENCY: usize = 8;

pub(super) enum CloudMsg {
    Job(Box<TranscribeJob>),
    /// 작업 스레드 하나가 끝남
    Done,
}

struct Dispatcher {
    queue: VecDeque<Box<TranscribeJob>>,
    active: usize,
    ffmpeg: Option<(Option<PathBuf>, FfmpegTools)>,
    clients: Vec<(CloudProvider, String, CloudClient)>,
}

pub(super) fn run(rx: Receiver<CloudMsg>, self_tx: Sender<CloudMsg>, tx: Sender<TaskEvent>, notify: Notify) {
    let mut d = Dispatcher { queue: VecDeque::new(), active: 0, ffmpeg: None, clients: Vec::new() };
    // 이 스레드가 self_tx 를 들고 있어 채널이 닫히지 않으므로, 앱 종료 시 프로세스와 함께 끝난다.
    for msg in rx {
        match msg {
            CloudMsg::Job(job) => d.queue.push_back(job),
            CloudMsg::Done => d.active = d.active.saturating_sub(1),
        }
        d.dispatch(&self_tx, &tx, &notify);
    }
}

impl Dispatcher {
    fn dispatch(&mut self, self_tx: &Sender<CloudMsg>, tx: &Sender<TaskEvent>, notify: &Notify) {
        while let Some(front) = self.queue.front() {
            // 대기 중 취소된 작업은 바로 취소 처리
            if front.cancel.load(Ordering::Relaxed) {
                if let Some(job) = self.queue.pop_front() {
                    let _ = tx.send(TaskEvent::Cancelled { id: job.id });
                    notify();
                }
                continue;
            }
            let limit = match &front.engine {
                JobEngine::Cloud { settings, .. } => (settings.max_concurrent_requests as usize).clamp(1, MAX_CLOUD_CONCURRENCY),
                JobEngine::Local { .. } => 1,
            };
            if self.active >= limit {
                break;
            }
            let Some(job) = self.queue.pop_front() else { break };
            let prepared = self.prepare(&job);
            self.active += 1;
            let (self_tx, tx, notify) = (self_tx.clone(), tx.clone(), Arc::clone(notify));
            let spawned = std::thread::Builder::new().name(format!("cloud-job-{}", job.id)).spawn(move || {
                let started = Instant::now();
                let result = prepared.and_then(|(tools, client)| process(&job, &tools, &client, &tx, &notify));
                let _ = tx.send(finish_event(&job, result, started.elapsed()));
                notify();
                let _ = self_tx.send(CloudMsg::Done);
            });
            if let Err(err) = spawned {
                tracing::error!(error = %err, "Cloud 작업 스레드 생성 실패");
                self.active -= 1;
            }
        }
    }

    /// FFmpeg 와 HTTP 클라이언트(연결 풀)를 캐시해 재사용한다.
    fn prepare(&mut self, job: &TranscribeJob) -> Result<(FfmpegTools, CloudClient), JobError> {
        let tools = match &self.ffmpeg {
            Some((dir, tools)) if *dir == job.ffmpeg_dir => tools.clone(),
            _ => {
                let tools = FfmpegTools::locate(job.ffmpeg_dir.as_deref())?;
                self.ffmpeg = Some((job.ffmpeg_dir.clone(), tools.clone()));
                tools
            }
        };
        let JobEngine::Cloud { settings, .. } = &job.engine else {
            return Err(JobError::Failed("Cloud 작업이 아닙니다".to_owned()));
        };
        let provider = settings.provider;
        let url = &settings.active().base_url;
        let client = match self.clients.iter().find(|(p, u, _)| *p == provider && u == url) {
            Some((_, _, c)) => c.clone(),
            None => {
                let c = CloudClient::new(provider, url)?;
                self.clients.push((provider, url.clone(), c.clone()));
                c
            }
        };
        Ok((tools, client))
    }
}

fn process(
    job: &TranscribeJob,
    tools: &FfmpegTools,
    client: &CloudClient,
    tx: &Sender<TaskEvent>,
    notify: &Notify,
) -> Result<PathBuf, JobError> {
    let JobEngine::Cloud { settings, api_key } = &job.engine else {
        return Err(JobError::Failed("Cloud 작업이 아닙니다".to_owned()));
    };
    let raw = |stage: Stage, progress: f32| {
        let _ = tx.send(TaskEvent::Progress { id: job.id, stage, progress });
        notify();
    };
    // URL 작업이면 오디오를 임시 WAV 로 받는다 (input 이 사라질 때 자동 삭제)
    let input = super::input::prepare(job, tools, &raw)?;
    let is_url = job.url.is_some();
    let send = |stage: Stage, p: f32| raw(stage, if is_url { super::input::remap_url(p) } else { p });
    send(Stage::Uploading, 0.0);
    let transcript = client.transcribe_file(
        tools,
        input.path(),
        input.duration,
        api_key,
        settings,
        job.language,
        &job.cancel,
        |p| send(Stage::Uploading, p * TRANSCRIBE_END),
    )?;
    if job.cancel.load(Ordering::Relaxed) {
        return Err(JobError::Cancelled);
    }
    send(Stage::Saving, TRANSCRIBE_END);
    Ok(txt::save_txt_named(&job.output_dir, &input.output_stem, &transcript)?)
}
