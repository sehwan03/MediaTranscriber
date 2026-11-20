use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};

use egui::RichText;

use super::state::{AppState, Discovery, DownloadState, FfmpegStatus, ToolStatus, KeyCheck, ModelOp, ModelOpKind, StatusLevel, View};
use crate::config::secret;
use crate::config::{AppPaths, CloudProvider, Settings};
use crate::media::format_duration;
use crate::speech::cloud::CloudClient;
use crate::media::worker::{MediaEvent, MediaRequest, MediaWorker};
use crate::speech::model_worker::ModelWorker;
use crate::system::SystemProfile;
use crate::task::{BenchmarkJob, TaskManager};
use crate::ui;
use crate::ui::theme;
use crate::url::{UrlError, UrlVideo, YtDlp};

/// eframe 애플리케이션 루트.
pub struct MediaTranscriberApp {
    state: AppState,
    /// 미디어 검사 스레드. 생성 실패 시 None (오류를 상태 표시줄에 표시).
    media: Option<MediaWorker>,
    /// 변환(오디오 추출 + Whisper) 스레드.
    tasks: Option<TaskManager>,
    /// 모델 다운로드/검증.
    models: ModelWorker,
    /// API Key 확인 결과 채널.
    key_tx: Sender<(CloudProvider, Result<(), String>)>,
    key_rx: Receiver<(CloudProvider, Result<(), String>)>,
    system_tx: Sender<SystemProfile>,
    system_rx: Receiver<SystemProfile>,
    install_tx: Sender<InstallEvent>,
    install_rx: Receiver<InstallEvent>,
    /// URL 변환: yt-dlp 확인/설치, 영상 탐색 결과
    url_tx: Sender<UrlEvent>,
    url_rx: Receiver<UrlEvent>,
    /// 탐색 요청 번호 (취소 후 늦게 도착한 이전 결과를 무시)
    discover_gen: u64,
    /// 영상 받기 작업 스레드 (처음 요청할 때 만든다. 한 번에 하나씩 받는다)
    download_jobs: Option<Sender<DownloadJob>>,
    download_tx: Sender<DownloadEvent>,
    download_rx: Receiver<DownloadEvent>,
}

struct DownloadJob {
    request: super::state::DownloadRequest,
    auth: crate::url::UrlAuth,
    out_dir: PathBuf,
    temp_dir: PathBuf,
    ytdlp_dir: PathBuf,
    ffmpeg_dir: Option<PathBuf>,
}

enum DownloadEvent {
    Progress { id: u64, progress: f32, merging: bool },
    Finished { id: u64, result: Result<PathBuf, UrlError> },
}

enum UrlEvent {
    YtDlp(Result<String, String>),
    InstallProgress(u64, u64),
    InstallFinished(Result<String, String>),
    Discovered { generation: u64, result: Result<Vec<UrlVideo>, UrlError> },
}

enum InstallEvent {
    Progress(u64, u64),
    Finished(Result<String, String>),
}

impl MediaTranscriberApp {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: AppPaths, settings: Settings) -> Self {
        theme::apply(&cc.egui_ctx);
        let mut state = AppState::new(paths, settings);
        if !theme::install_fonts(&cc.egui_ctx) {
            state.set_status(
                StatusLevel::Warning,
                "한글 글꼴(맑은 고딕)을 찾지 못해 한글이 표시되지 않을 수 있습니다.",
            );
        }

        let repaint_ctx = cc.egui_ctx.clone();
        let media = match MediaWorker::spawn(state.settings.ffmpeg_dir.clone(), move || {
            repaint_ctx.request_repaint()
        }) {
            Ok(worker) => Some(worker),
            Err(err) => {
                tracing::error!(error = %err, "미디어 작업 스레드 생성 실패");
                state.set_status(StatusLevel::Error, format!("미디어 작업 스레드 생성 실패: {err}"));
                None
            }
        };
        let repaint_ctx = cc.egui_ctx.clone();
        let tasks = match TaskManager::spawn(move || repaint_ctx.request_repaint()) {
            Ok(worker) => Some(worker),
            Err(err) => {
                tracing::error!(error = %err, "변환 작업 스레드 생성 실패");
                state.set_status(StatusLevel::Error, format!("변환 작업 스레드 생성 실패: {err}"));
                None
            }
        };
        let repaint_ctx = cc.egui_ctx.clone();
        let models = ModelWorker::new(move || repaint_ctx.request_repaint());

        // 사용자가 이전에 "이 PC에 기억"을 선택한 경우에만 저장된 키가 있다.
        for provider in CloudProvider::ALL {
            match secret::load_api_key(provider) {
                Ok(Some(key)) => {
                    let slot = state.key_slot_mut(provider);
                    slot.key = Some(key);
                    slot.saved = true;
                    tracing::info!(provider = provider.label(), "저장된 API Key 를 불러옴");
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(provider = provider.label(), error = %err, "저장된 API Key 읽기 실패");
                    state.set_status(
                        StatusLevel::Warning,
                        format!("저장된 {} API Key 를 읽지 못했습니다: {err}", provider.label()),
                    );
                }
            }
        }
        let (key_tx, key_rx) = mpsc::channel();
        let (system_tx, system_rx) = mpsc::channel();
        let (install_tx, install_rx) = mpsc::channel();
        let (url_tx, url_rx) = mpsc::channel();
        let (download_tx, download_rx) = mpsc::channel();

        // 지난 실행이 비정상 종료로 남긴 URL 임시 오디오 정리
        let removed = crate::utils::tempfile::cleanup_stale(&state.paths.temp_dir, "url");
        if removed > 0 {
            tracing::info!(removed, "이전 실행의 임시 파일 정리");
        }

        let mut app = Self {
            state,
            media,
            tasks,
            models,
            key_tx,
            key_rx,
            system_tx,
            system_rx,
            install_tx,
            install_rx,
            url_tx,
            url_rx,
            discover_gen: 0,
            download_jobs: None,
            download_tx,
            download_rx,
        };
        // 최초 실행 시 하드웨어 검사 (백그라운드)
        app.collect_system_info(&cc.egui_ctx);
        app
    }

    fn collect_system_info(&mut self, ctx: &egui::Context) {
        let tx = self.system_tx.clone();
        let ctx = ctx.clone();
        let models_dir = self.state.paths.models_dir.clone();
        let spawned = std::thread::Builder::new().name("system-info".to_owned()).spawn(move || {
            let _ = tx.send(crate::system::collect(&models_dir));
            ctx.request_repaint();
        });
        if let Err(err) = spawned {
            tracing::error!(error = %err, "시스템 정보 스레드 생성 실패");
        }
    }

    /// FFmpeg 자동 설치 요청/진행 처리 (다운로드는 백그라운드 스레드).
    fn sync_ffmpeg_install(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.install_rx.try_recv() {
            match event {
                InstallEvent::Progress(done, total) => {
                    if let Some(p) = self.state.ffmpeg_install.as_mut() {
                        p.done = done;
                        p.total = total;
                    }
                }
                InstallEvent::Finished(result) => {
                    self.state.ffmpeg_install = None;
                    match result {
                        Ok(version) => {
                            self.state.set_status(StatusLevel::Info, format!("FFmpeg {version} 설치 완료"));
                            // 미디어 검사 스레드가 새로 설치된 FFmpeg 를 다시 찾게 한다
                            self.state.ffmpeg_relocate_requested = true;
                        }
                        Err(msg) => self.state.set_status(StatusLevel::Error, format!("FFmpeg 설치 실패: {msg}")),
                    }
                }
            }
        }
        if !std::mem::take(&mut self.state.ffmpeg_install_requested) || self.state.ffmpeg_install.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let dir = self.state.paths.ffmpeg_dir.clone();
        let tx = self.install_tx.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("ffmpeg-install".to_owned()).spawn(move || {
            let progress_tx = tx.clone();
            let progress_ctx = ctx.clone();
            let result = crate::media::installer::install(&dir, &thread_cancel, |done, total| {
                let _ = progress_tx.send(InstallEvent::Progress(done, total));
                progress_ctx.request_repaint();
            })
            .map(|tools| tools.version().to_owned())
            .map_err(|e| e.to_string());
            if let Err(e) = &result {
                tracing::warn!(error = %e, "FFmpeg 자동 설치 실패");
            }
            let _ = tx.send(InstallEvent::Finished(result));
            ctx.request_repaint();
        });
        match spawned {
            Ok(_) => self.state.ffmpeg_install = Some(crate::app::FfmpegInstall { done: 0, total: 0, cancel }),
            Err(err) => self.state.set_status(StatusLevel::Error, format!("설치 스레드 생성 실패: {err}")),
        }
    }

    /// URL 변환: yt-dlp 확인·설치, 영상 탐색 (모두 백그라운드 스레드, UI 는 멈추지 않음)
    fn sync_url(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.url_rx.try_recv() {
            let u = &mut self.state.url_ui;
            match event {
                UrlEvent::YtDlp(Ok(version)) => u.ytdlp = ToolStatus::Found { version },
                UrlEvent::YtDlp(Err(msg)) => u.ytdlp = ToolStatus::Missing(msg),
                UrlEvent::InstallProgress(done, total) => {
                    if let Some(p) = u.ytdlp_install.as_mut() {
                        p.done = done;
                        p.total = total;
                    }
                }
                UrlEvent::InstallFinished(result) => {
                    u.ytdlp_install = None;
                    match result {
                        Ok(version) => {
                            u.ytdlp = ToolStatus::Found { version: version.clone() };
                            self.state.set_status(StatusLevel::Info, format!("URL 변환 구성 요소 설치 완료 ({version})"));
                        }
                        Err(msg) => self.state.set_status(StatusLevel::Error, format!("URL 변환 구성 요소 설치 실패: {msg}")),
                    }
                }
                UrlEvent::Discovered { generation, result } => {
                    if generation != self.discover_gen {
                        continue;
                    }
                    match result {
                        Ok(videos) => {
                            let n = videos.len();
                            u.discovery = Discovery::Done { videos: videos.into_iter().map(|v| (v, false)).collect() };
                            self.state.set_status(StatusLevel::Info, format!("영상 {n}개를 찾았습니다. 변환할 영상을 선택하세요."));
                        }
                        Err(UrlError::Cancelled) => {
                            u.discovery = Discovery::Idle;
                            self.state.set_status(StatusLevel::Info, "영상 찾기를 취소했습니다.");
                        }
                        Err(err) => {
                            let msg = err.to_string();
                            u.discovery = Discovery::Failed(msg.clone());
                            self.state.set_status(StatusLevel::Warning, msg);
                        }
                    }
                }
            }
        }

        // yt-dlp 확인 (yt-dlp.exe 는 시작이 1~2초 걸려 백그라운드에서)
        let u = &mut self.state.url_ui;
        if std::mem::take(&mut u.ytdlp_check_requested) && !matches!(u.ytdlp, ToolStatus::Checking) {
            u.ytdlp = ToolStatus::Checking;
            let dir = self.state.paths.ytdlp_dir.clone();
            self.spawn_url_thread(ctx, "ytdlp-check", move || {
                UrlEvent::YtDlp(YtDlp::locate(Some(&dir)).and_then(|y| y.version()).map_err(|e| e.to_string()))
            });
        }

        let u = &mut self.state.url_ui;
        if std::mem::take(&mut u.ytdlp_install_requested) && u.ytdlp_install.is_none() {
            let cancel = Arc::new(AtomicBool::new(false));
            u.ytdlp_install = Some(crate::app::FfmpegInstall { done: 0, total: 0, cancel: Arc::clone(&cancel) });
            let dir = self.state.paths.ytdlp_dir.clone();
            let (tx, progress_ctx) = (self.url_tx.clone(), ctx.clone());
            self.spawn_url_thread(ctx, "ytdlp-install", move || {
                let result = crate::url::installer::install(&dir, &cancel, |done, total| {
                    let _ = tx.send(UrlEvent::InstallProgress(done, total));
                    progress_ctx.request_repaint();
                });
                if let Err(e) = &result {
                    tracing::warn!(error = %e, "yt-dlp 자동 설치 실패");
                }
                UrlEvent::InstallFinished(result.map(|(_, v)| v).map_err(|e| e.to_string()))
            });
        }

        let u = &mut self.state.url_ui;
        if std::mem::take(&mut u.discover_requested) && !matches!(u.discovery, Discovery::Running { .. }) {
            let url = u.input.trim().to_owned();
            // 주소 형식은 스레드를 띄우기 전에 확인한다 (http/https 만, 공백·제어문자 금지)
            if let Err(err) = crate::url::safety::validate_url(&url) {
                u.discovery = Discovery::Failed(err.to_string());
                return;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            u.discovery = Discovery::Running { cancel: Arc::clone(&cancel) };
            self.discover_gen += 1;
            let generation = self.discover_gen;
            let dir = self.state.paths.ytdlp_dir.clone();
            let auth = self.state.settings.url_auth.clone();
            let ffmpeg_dir = self.state.settings.ffmpeg_dir.clone();
            tracing::info!(url = %crate::url::safety::redact_url(&url), "영상 찾기 시작");
            self.spawn_url_thread(ctx, "url-discover", move || {
                let result = YtDlp::locate(Some(&dir)).and_then(|y| crate::url::discover(&y, &url, &auth, &cancel)).map(|mut videos| {
                    // 직접 MP4 등 길이를 못 준 항목은 ffprobe 로 보충
                    if let Ok(tools) = crate::media::FfmpegTools::locate(ffmpeg_dir.as_deref()) {
                        crate::url::fill_missing_durations(&mut videos, &tools, &cancel);
                    }
                    videos
                });
                let result = match result {
                    Ok(_) if cancel.load(std::sync::atomic::Ordering::Relaxed) => Err(UrlError::Cancelled),
                    other => other,
                };
                match &result {
                    Ok(v) => tracing::info!(count = v.len(), "영상 찾기 완료"),
                    Err(e) => tracing::warn!(error = %e, "영상 찾기 실패"),
                }
                UrlEvent::Discovered { generation, result }
            });
        }
    }

    /// 영상 받기: 결과 반영, 새 요청을 작업 스레드로 전달
    fn sync_downloads(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.download_rx.try_recv() {
            match event {
                DownloadEvent::Progress { id, progress, merging } => {
                    if let Some(d) = self.state.downloads.iter_mut().find(|d| d.id == id) {
                        d.state = DownloadState::Running { progress, merging };
                    }
                }
                DownloadEvent::Finished { id, result } => {
                    let Some(d) = self.state.downloads.iter_mut().find(|d| d.id == id) else { continue };
                    let title = d.title.clone();
                    match result {
                        Ok(path) => {
                            d.state = DownloadState::Done(path);
                            self.state.set_status(StatusLevel::Info, format!("영상을 저장했습니다: {title}"));
                        }
                        Err(UrlError::Cancelled) => d.state = DownloadState::Cancelled,
                        Err(err) => {
                            let msg = err.to_string();
                            d.state = DownloadState::Failed(msg.clone());
                            self.state.set_status(StatusLevel::Error, format!("영상 저장 실패: {msg}"));
                        }
                    }
                }
            }
        }

        let requests = std::mem::take(&mut self.state.download_requests);
        if requests.is_empty() {
            return;
        }
        if self.download_jobs.is_none() {
            self.download_jobs = self.spawn_download_worker(ctx);
        }
        let Some(jobs) = &self.download_jobs else {
            for r in requests {
                if let Some(d) = self.state.downloads.iter_mut().find(|d| d.id == r.id) {
                    d.state = DownloadState::Failed("작업 스레드를 만들 수 없습니다".to_owned());
                }
            }
            return;
        };
        for request in requests {
            let job = DownloadJob {
                request,
                auth: self.state.settings.url_auth.clone(),
                out_dir: self.state.output_dir().to_path_buf(),
                temp_dir: self.state.paths.temp_dir.clone(),
                ytdlp_dir: self.state.paths.ytdlp_dir.clone(),
                ffmpeg_dir: self.state.settings.ffmpeg_dir.clone(),
            };
            if jobs.send(job).is_err() {
                self.download_jobs = None;
                self.state.set_status(StatusLevel::Error, "영상 받기 작업 스레드가 멈췄습니다. 다시 시도하세요.");
                return;
            }
        }
    }

    fn spawn_download_worker(&self, ctx: &egui::Context) -> Option<Sender<DownloadJob>> {
        let (job_tx, job_rx) = mpsc::channel::<DownloadJob>();
        let (events, ctx) = (self.download_tx.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("video-download".to_owned()).spawn(move || {
            for job in job_rx {
                let id = job.request.id;
                let cancel = &job.request.cancel;
                let result = if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    Err(UrlError::Cancelled)
                } else {
                    let _ = events.send(DownloadEvent::Progress { id, progress: 0.0, merging: false });
                    ctx.request_repaint();
                    let mut last = -1.0f32;
                    YtDlp::locate(Some(&job.ytdlp_dir)).and_then(|y| {
                        let tools = crate::media::FfmpegTools::locate(job.ffmpeg_dir.as_deref())
                            .map_err(|e| UrlError::Failed(e.to_string()))?;
                        crate::url::download::download_video(
                            &y,
                            &tools,
                            &job.request.video,
                            &job.auth,
                            &job.out_dir,
                            &job.temp_dir,
                            cancel,
                            |stage, progress| {
                                let merging = stage == crate::url::download::DownloadStage::Merging;
                                if progress - last >= 0.005 || merging || progress >= 1.0 {
                                    last = progress;
                                    let _ = events.send(DownloadEvent::Progress { id, progress, merging });
                                    ctx.request_repaint();
                                }
                            },
                        )
                    })
                };
                if let Err(e) = &result {
                    tracing::warn!(id, error = %e, "영상 받기 끝 (실패/취소)");
                }
                let _ = events.send(DownloadEvent::Finished { id, result });
                ctx.request_repaint();
            }
        });
        match spawned {
            Ok(_) => Some(job_tx),
            Err(err) => {
                tracing::error!(error = %err, "영상 받기 스레드 생성 실패");
                None
            }
        }
    }

    fn spawn_url_thread(&self, ctx: &egui::Context, name: &str, work: impl FnOnce() -> UrlEvent + Send + 'static) {
        let (tx, ctx) = (self.url_tx.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name(name.to_owned()).spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
        if let Err(err) = spawned {
            tracing::error!(error = %err, name, "스레드 생성 실패");
        }
    }

    /// 시스템 정보 수신, 새로고침/성능 측정 요청 처리.
    fn sync_system(&mut self, ctx: &egui::Context) {
        while let Ok(profile) = self.system_rx.try_recv() {
            self.state.system = Some(profile);
        }
        if std::mem::take(&mut self.state.system_refresh_requested) {
            self.collect_system_info(ctx);
        }
        if let Some(model) = self.state.benchmark_requested.take() {
            let Some(tasks) = &self.tasks else { return };
            if self.state.model_file_state(model) != crate::speech::model::ModelFileState::Installed {
                self.state.set_status(StatusLevel::Warning, format!("{} 모델을 먼저 다운로드하세요.", model.label()));
                return;
            }
            let job = BenchmarkJob {
                model,
                model_path: crate::speech::model::model_path(&self.state.paths.models_dir, model),
                sample: self.state.benchmark_sample(),
                ffmpeg_dir: self.state.settings.ffmpeg_dir.clone(),
            };
            match tasks.submit_benchmark(job) {
                Ok(()) => {
                    self.state.benchmark_running = Some(model);
                    self.state.set_status(StatusLevel::Info, format!("{} 모델 성능 측정 중… (30초 샘플)", model.label()));
                }
                Err(e) => self.state.set_status(StatusLevel::Error, e),
            }
        }
    }

    /// API Key 확인 요청을 백그라운드 스레드에서 처리한다 (GUI 스레드에서 HTTP 금지).
    fn sync_key_check(&mut self, ctx: &egui::Context) {
        while let Ok((provider, result)) = self.key_rx.try_recv() {
            self.state.key_slot_mut(provider).check = match result {
                Ok(()) => KeyCheck::Valid,
                Err(msg) => KeyCheck::Invalid(msg),
            };
        }
        let Some(provider) = self.state.key_check_requested.take() else { return };
        let Some(key) = self.state.key_slot(provider).key.clone() else { return };
        let base_url = self.state.settings.cloud.provider_settings(provider).base_url.clone();
        let tx = self.key_tx.clone();
        let ctx = ctx.clone();
        self.state.key_slot_mut(provider).check = KeyCheck::Checking;
        let spawned = std::thread::Builder::new().name("api-key-check".to_owned()).spawn(move || {
            let result = CloudClient::new(provider, &base_url)
                .and_then(|client| client.validate_key(&key))
                .map_err(|e| e.to_string());
            match &result {
                Ok(()) => tracing::info!(provider = provider.label(), "API Key 확인 성공"),
                Err(e) => tracing::warn!(provider = provider.label(), error = %e, "API Key 확인 실패"),
            }
            let _ = tx.send((provider, result));
            ctx.request_repaint();
        });
        if let Err(err) = spawned {
            self.state.key_slot_mut(provider).check = KeyCheck::Invalid(format!("확인 스레드 생성 실패: {err}"));
        }
    }

    /// 변환 작업/모델 작업 결과 반영과 요청 전달.
    fn sync_workers(&mut self) {
        while let Some(event) = self.models.try_recv() {
            self.state.apply_model_event(event);
        }
        for (model, kind) in std::mem::take(&mut self.state.model_requests) {
            if self.state.model_op(model).is_some() {
                continue;
            }
            let dir = self.state.paths.models_dir.clone();
            let cancel = match kind {
                ModelOpKind::Download => self.models.start_download(dir, model),
                ModelOpKind::Verify => self.models.start_verify(dir, model),
            };
            self.state.model_ops.push(ModelOp { model, kind, done: 0, total: 0, cancel });
        }

        let Some(tasks) = &self.tasks else { return };
        while let Some(event) = tasks.try_recv() {
            self.state.apply_task_event(event);
        }
        if std::mem::take(&mut self.state.start_requested) {
            // Auto 판단 직전에 사용 가능 RAM 을 다시 읽는다 (가벼운 호출).
            if let (Some(profile), Some(avail)) = (self.state.system.as_mut(), crate::system::memory::available_ram()) {
                profile.available_ram = avail;
            }
            if let Some(reason) = self.state.start_blocker() {
                self.state.set_status(StatusLevel::Warning, reason);
                return;
            }
            // Cloud 는 파일을 외부 서버로 보내므로 매번 사용자 동의를 받는다.
            if self.state.uses_cloud() && !std::mem::take(&mut self.state.consent_given) {
                self.state.consent_pending = Some(self.state.startable_summary());
                return;
            }
            let jobs = self.state.build_jobs();
            let count = jobs.len();
            for job in jobs {
                if let Err(err) = tasks.submit(job) {
                    self.state.set_status(StatusLevel::Error, err);
                    return;
                }
            }
            tracing::info!(count, "변환 작업 등록");
            self.state.set_status(StatusLevel::Info, format!("{count}개 파일 변환을 시작합니다."));
        }
    }

    /// 백그라운드 결과 반영 + 새 요청 전달. 블로킹 작업은 하지 않는다.
    fn sync_media_worker(&mut self) {
        let Some(media) = &self.media else { return };
        while let Some(event) = media.try_recv() {
            match event {
                MediaEvent::Tools(result) => self.state.apply_ffmpeg_status(result),
                MediaEvent::Probed { id, result } => self.state.apply_probe_result(id, result),
            }
        }
        if std::mem::take(&mut self.state.ffmpeg_relocate_requested) {
            self.state.ffmpeg = FfmpegStatus::Searching;
            media.send(MediaRequest::Locate(self.state.settings.ffmpeg_dir.clone()));
        }
        for (id, path) in self.state.take_pending_probes() {
            media.send(MediaRequest::Probe { id, path });
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        let (added, skipped) = self.state.add_paths(dropped);
        ui::file_panel::report_added(&mut self.state, added, skipped);
    }
}

impl eframe::App for MediaTranscriberApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_dropped_files(ui.ctx());
        self.sync_media_worker();
        self.sync_workers();

        let bar = |x: i8, y: i8| egui::Frame::new().fill(theme::HEADER).inner_margin(egui::Margin::symmetric(x, y));
        egui::Panel::top("header")
            .frame(bar(20, 12))
            .show_separator_line(false)
            .show(ui, |ui| header(ui, &mut self.state));
        egui::Panel::bottom("status_bar")
            .frame(bar(20, 6))
            .show_separator_line(false)
            .show(ui, |ui| status_bar(ui, &self.state));

        match self.state.view {
            View::Main => {
                egui::Panel::right("side_panel")
                    .resizable(false)
                    .exact_size(372.0)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left: 8, right: 20, top: 16, bottom: 12 }))
                    .show(ui, |ui| {
                        egui::Panel::bottom("side_footer")
                            .frame(egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 0, top: 12, bottom: 0 }))
                            .show_separator_line(false)
                            .resizable(false)
                            .show(ui, |ui| ui::task_panel::show_footer(ui, &mut self.state));
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| ui::task_panel::show(ui, &mut self.state));
                    });
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left: 20, right: 8, top: 16, bottom: 12 }))
                    .show(ui, |ui| ui::file_panel::show(ui, &mut self.state));
            }
            View::Settings => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(20, 16)))
                    .show(ui, |ui| ui::settings_panel::show(ui, &mut self.state));
            }
        }

        // 동의 화면에서 "전송"을 누르면 다음 sync 에서 바로 작업을 등록한다.
        consent_dialog(ui.ctx(), &mut self.state);

        // 이번 프레임에 UI 에서 생긴 요청(파일 추가, 변환 시작 등)을 바로 전달한다.
        self.sync_media_worker();
        self.sync_workers();
        self.sync_key_check(ui.ctx());
        self.sync_system(ui.ctx());
        self.sync_ffmpeg_install(ui.ctx());
        self.sync_url(ui.ctx());
        self.sync_downloads(ui.ctx());
        self.state.persist_settings_if_changed();
    }

    /// 종료 시 진행 중인 작업을 취소해 FFmpeg/추론이 즉시 멈추도록 한다.
    /// 이미 저장된 TXT 파일은 그대로 남는다.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.state.cancel_all();
        for op in &self.state.model_ops {
            op.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(install) = &self.state.ffmpeg_install {
            install.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(install) = &self.state.url_ui.ytdlp_install {
            install.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Discovery::Running { cancel } = &self.state.url_ui.discovery {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if self.state.has_active_downloads() {
            self.state.cancel_downloads();
            // 받는 프로세스가 끝나고 받던 조각이 지워질 시간을 잠깐 준다 (남으면 다음 실행 때 정리)
            std::thread::sleep(std::time::Duration::from_millis(600));
        }
        tracing::info!("종료 요청: 진행 중인 작업 취소");
    }
}

/// Cloud 전송 전 동의 화면. 동의하지 않으면 아무 파일도 전송하지 않는다.
fn consent_dialog(ctx: &egui::Context, state: &mut AppState) {
    let Some((count, total)) = state.consent_pending else { return };
    let mut decision: Option<bool> = None;
    let response = egui::Modal::new(egui::Id::unique("cloud_consent")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.label(RichText::new("Cloud 전송 동의").font(theme::bold(18.0)).color(theme::TEXT));
        ui.add_space(8.0);
        let provider = state.settings.cloud.provider;
        let name = provider.label();
        ui.label(format!(
            "선택한 {count}개 파일(총 길이 약 {})의 음성을 {name} 서버로 전송해 텍스트로 변환합니다.",
            format_duration(total)
        ));
        ui.add_space(4.0);
        ui.label(RichText::new(format!("모델: {}", state.settings.cloud.active().model)).weak());
        if state.settings.engine_mode == crate::config::EngineMode::Auto {
            ui.label(RichText::new("Auto 모드가 이 PC 성능을 분석해 Cloud 를 추천했습니다.").weak());
        }
        ui.add_space(8.0);
        ui.label("• 음성 데이터가 인터넷을 통해 외부 서버로 전송됩니다.");
        match provider {
            CloudProvider::OpenAi => {
                ui.label("• 사용량에 따라 입력한 API Key 의 OpenAI 계정에 요금이 청구됩니다.");
            }
            CloudProvider::Gemini => {
                ui.label("• 유료 등급이면 사용량에 따라 Google 계정에 요금이 청구됩니다.");
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "• 무료 등급(Unpaid)으로 보낸 음성과 결과는 Google 제품 개선에 사용되며, \
                     사람 검토자가 읽을 수 있습니다 (Gemini API 약관).",
                );
                ui.label("• 무료 등급은 EEA·영국·스위스 사용자 대상 서비스에 사용할 수 없습니다.");
            }
        }
        ui.label("• 민감하거나 개인적인 내용이 있다면 Local 모드를 사용하세요.");
        ui.add_space(12.0);
        // 클릭 한 번(오클릭 포함)으로 전송되지 않도록 확인 체크 후에만 전송 버튼을 활성화한다.
        ui.checkbox(&mut state.consent_checked, "위 내용을 확인했으며 전송에 동의합니다");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new("취소").min_size(egui::vec2(96.0, 36.0))).clicked() {
                decision = Some(false);
            }
            let send = ui.add_enabled(
                state.consent_checked,
                theme::primary_button("동의하고 전송").min_size(egui::vec2(160.0, 36.0)),
            );
            if send.clicked() && state.consent_checked {
                decision = Some(true);
            }
        });
    });
    if response.should_close() && decision.is_none() {
        decision = Some(false); // Esc / 바깥 클릭 = 취소
    }
    match decision {
        Some(true) => {
            tracing::info!(count, "Cloud 전송 동의");
            state.consent_pending = None;
            state.consent_checked = false;
            state.consent_given = true;
            state.start_requested = true;
        }
        Some(false) => {
            tracing::info!("Cloud 전송 취소 (동의하지 않음)");
            state.consent_pending = None;
            state.consent_checked = false;
            state.set_status(StatusLevel::Info, "Cloud 전송을 취소했습니다. 파일은 전송되지 않았습니다.");
        }
        None => {}
    }
}

fn header(ui: &mut egui::Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        // 로고 + 이름
        let (logo, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
        ui.painter().rect_filled(logo, egui::CornerRadius::same(9), theme::ACCENT);
        theme::paint_icon(ui.painter(), logo.shrink(7.0), theme::Icon::Wave, egui::Color32::WHITE);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::APP_NAME).font(theme::bold(17.0)).color(theme::TEXT));
                ui.label(RichText::new(format!("v{}", crate::APP_VERSION)).size(11.5).color(theme::TEXT_FAINT));
            });
            ui.label(RichText::new("음성·동영상 텍스트 변환").size(12.0).color(theme::TEXT_DIM));
        });
        ui.add_space(24.0);

        // 화면 전환 탭
        let tabs_width = 200.0;
        ui.allocate_ui(egui::vec2(tabs_width, 32.0), |ui| {
            ui.set_width(tabs_width);
            theme::segmented(ui, &mut state.view, &[(View::Main, "변환"), (View::Settings, "설정")], true);
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // 전체 상태 배지
            let active = state.files.iter().filter(|f| f.job.is_active()).count();
            if active > 0 {
                theme::pill(ui, &format!("처리 중 {active}"), theme::Tone::Info);
            } else if state.ffmpeg_install.is_some() {
                theme::pill(ui, "FFmpeg 설치 중", theme::Tone::Info);
            } else {
                match &state.ffmpeg {
                    FfmpegStatus::Searching => {
                        theme::pill(ui, "준비 중", theme::Tone::Neutral);
                    }
                    FfmpegStatus::Missing(msg) => {
                        theme::pill(ui, "FFmpeg 필요", theme::Tone::Warn).on_hover_text(msg);
                    }
                    FfmpegStatus::Found { .. } => {
                        theme::pill(ui, "준비됨", theme::Tone::Success);
                    }
                }
            }
            ui.add_space(8.0);
            let mut info = Vec::new();
            if let FfmpegStatus::Found { version } = &state.ffmpeg {
                info.push(format!("FFmpeg {version}"));
            }
            match &state.system {
                Some(p) => {
                    info.push(format!("CPU {}스레드", p.logical_cores));
                    info.push(format!("RAM {:.1} GB 여유", p.available_ram as f64 / (1024.0 * 1024.0 * 1024.0)));
                }
                None => info.push(format!("CPU {}스레드", state.logical_cores)),
            }
            ui.label(RichText::new(info.join("  ·  ")).size(12.0).color(theme::TEXT_FAINT));
        });
    });
}

fn status_bar(ui: &mut egui::Ui, state: &AppState) {
    ui.horizontal(|ui| {
        let (text, color) = match &state.status {
            Some(msg) => (
                msg.text.as_str(),
                match msg.level {
                    StatusLevel::Info => theme::TEXT_DIM,
                    StatusLevel::Warning => theme::WARN,
                    StatusLevel::Error => theme::DANGER,
                },
            ),
            None => ("준비", theme::TEXT_FAINT),
        };
        let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 3.5, color);
        ui.label(RichText::new(text).size(12.5).color(color));
    });
}

