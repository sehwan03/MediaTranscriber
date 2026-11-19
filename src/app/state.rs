use std::collections::HashSet;
use std::path::{Path, PathBuf};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::config::{AppPaths, CloudProvider, EngineMode, Settings, WhisperModel};
use crate::media::MediaInfo;
use crate::speech::model::{self, ModelFileState};
use crate::speech::model_worker::ModelEvent;
use crate::config::secret::{self, ApiKey};
use crate::engine_selector::{Choice, Recommendation, SelectorInput, recommend};
use crate::system::SystemProfile;
use crate::task::{JobEngine, Stage, TaskEvent, TranscribeJob};
use zeroize::Zeroize;

/// 확장자 기반 1차 필터. 실제 미디어 유효성은 등록 후 ffprobe 로 다시 검사한다.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    // 동영상
    "mp4", "mkv", "avi", "mov", "webm", //
    // 음성
    "mp3", "wav", "m4a", "flac", "aac", "ogg",
];

pub fn has_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| SUPPORTED_EXTENSIONS.iter().any(|s| s.eq_ignore_ascii_case(ext)))
}

/// 사용자가 등록한 입력 파일.
#[derive(Debug, Clone)]
pub struct InputFile {
    pub id: u64,
    pub path: PathBuf,
    /// 표시용 파일명 (매 프레임 계산하지 않도록 캐시).
    pub display_name: String,
    pub size_bytes: u64,
    pub probe: ProbeState,
    pub job: JobState,
    /// 작업에 실제 사용된 엔진 (작업 시작 시 기록)
    pub engine_label: Option<String>,
}

/// ffprobe 검사 상태.
#[derive(Debug, Clone)]
pub enum ProbeState {
    Pending,
    Ready(MediaInfo),
    Failed(String),
}

/// FFmpeg 탐색 상태.
#[derive(Debug, Clone)]
pub enum FfmpegStatus {
    Searching,
    Found { version: String },
    Missing(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Main,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub level: StatusLevel,
    pub text: String,
}

/// GUI 가 소유하는 상태. 백그라운드 작업 상태는 이후 단계에서 채널을 통해 반영한다.
pub struct AppState {
    pub paths: AppPaths,
    pub settings: Settings,
    /// 마지막으로 디스크에 저장된 설정. 변경 감지용.
    saved_settings: Settings,
    pub files: Vec<InputFile>,
    known_paths: HashSet<PathBuf>,
    pub view: View,
    pub status: Option<StatusMessage>,
    pub logical_cores: usize,
    pub ffmpeg: FfmpegStatus,
    /// 백그라운드 검사를 요청해야 하는 파일 (App 이 매 프레임 꺼내 MediaWorker 로 보낸다).
    pending_probes: Vec<(u64, PathBuf)>,
    /// FFmpeg 재탐색 요청 (설정에서 경로 변경 시).
    pub ffmpeg_relocate_requested: bool,
    next_file_id: u64,
    /// 모델 파일 설치 상태 (크기 기반 빠른 확인).
    model_files: [ModelFileState; 4],
    /// 진행 중인 모델 다운로드/검증.
    pub model_ops: Vec<ModelOp>,
    /// 이번 실행에서 SHA-256 검증을 통과한 모델.
    pub verified_models: Vec<WhisperModel>,
    /// 삭제 확인 대기 중인 모델 (버튼 두 번 클릭 확인).
    pub delete_confirm: Option<WhisperModel>,
    /// UI 에서 요청한 동작. App 이 프레임 끝에 꺼내 백그라운드 스레드로 전달한다.
    pub start_requested: bool,
    pub model_requests: Vec<(WhisperModel, ModelOpKind)>,

    /// 공급자별 API Key (메모리에만 보관, settings.json 에는 저장하지 않는다).
    pub keys: [KeySlot; 2],
    /// 설정 화면 입력란 버퍼. 적용 즉시 비우고 0 으로 덮어쓴다.
    pub api_key_input: String,
    pub remember_api_key: bool,
    /// 키 확인 요청 (백그라운드 스레드에서 처리)
    pub key_check_requested: Option<CloudProvider>,
    /// Cloud 전송 동의 대기 중 (파일 수, 총 길이)
    pub consent_pending: Option<(usize, Duration)>,
    /// 사용자가 동의 화면에서 "전송" 을 눌렀다.
    pub consent_given: bool,
    /// 동의 화면의 확인 체크박스 (열릴 때마다 해제 상태로 시작).
    pub consent_checked: bool,

    /// 완료된 결과 (작업 목록에서 지워도 유지)
    pub results: Vec<ResultEntry>,
    /// 이번 일괄 처리 통계 (모두 끝나면 요약 메시지 표시)
    batch: Option<BatchStats>,

    /// FFmpeg 자동 설치 진행 상태
    pub ffmpeg_install: Option<FfmpegInstall>,
    pub ffmpeg_install_requested: bool,

    /// 하드웨어 정보 (시작 시 백그라운드에서 수집)
    pub system: Option<SystemProfile>,
    pub system_refresh_requested: bool,
    /// 성능 측정 요청/진행 중인 모델
    pub benchmark_requested: Option<WhisperModel>,
    pub benchmark_running: Option<WhisperModel>,
}

/// FFmpeg 자동 설치 진행 상황.
pub struct FfmpegInstall {
    pub done: u64,
    pub total: u64,
    pub cancel: Arc<AtomicBool>,
}

/// 완료된 결과 하나.
#[derive(Debug, Clone)]
pub struct ResultEntry {
    pub source_name: String,
    pub output: PathBuf,
    pub engine: String,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, Copy, Default)]
struct BatchStats {
    done: usize,
    failed: usize,
    cancelled: usize,
}

/// 공급자 하나의 API Key 상태.
#[derive(Debug, Clone)]
pub struct KeySlot {
    pub key: Option<ApiKey>,
    /// Windows 자격 증명 관리자에 저장되어 있는지.
    pub saved: bool,
    pub check: KeyCheck,
}

impl Default for KeySlot {
    fn default() -> Self {
        Self { key: None, saved: false, check: KeyCheck::Unchecked }
    }
}

/// API Key 확인 상태.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyCheck {
    Unchecked,
    Checking,
    Valid,
    Invalid(String),
}

/// 진행 중인 모델 작업.
pub struct ModelOp {
    pub model: WhisperModel,
    pub kind: ModelOpKind,
    pub done: u64,
    pub total: u64,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelOpKind {
    Download,
    Verify,
}

/// 변환 작업 상태.
#[derive(Debug, Clone)]
pub enum JobState {
    Idle,
    Queued { cancel: Arc<AtomicBool> },
    Running { cancel: Arc<AtomicBool>, stage: Stage, progress: f32 },
    Completed { output: PathBuf, elapsed: Duration },
    Failed(String),
    Cancelled,
}

impl JobState {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued { .. } | Self::Running { .. })
    }

    fn cancel_flag(&self) -> Option<&Arc<AtomicBool>> {
        match self {
            Self::Queued { cancel } | Self::Running { cancel, .. } => Some(cancel),
            _ => None,
        }
    }
}

fn model_index(model: WhisperModel) -> usize {
    match model {
        WhisperModel::Tiny => 0,
        WhisperModel::Base => 1,
        WhisperModel::Small => 2,
        WhisperModel::Medium => 3,
    }
}

impl AppState {
    pub fn new(paths: AppPaths, settings: Settings) -> Self {
        let logical_cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let model_files = WhisperModel::ALL.map(|m| model::quick_check(&paths.models_dir, m));
        Self {
            model_files,
            model_ops: Vec::new(),
            verified_models: Vec::new(),
            delete_confirm: None,
            start_requested: false,
            model_requests: Vec::new(),
            keys: [KeySlot::default(), KeySlot::default()],
            api_key_input: String::new(),
            remember_api_key: false,
            key_check_requested: None,
            consent_pending: None,
            consent_given: false,
            consent_checked: false,
            results: Vec::new(),
            batch: None,
            ffmpeg_install: None,
            ffmpeg_install_requested: false,
            system: None,
            system_refresh_requested: false,
            benchmark_requested: None,
            benchmark_running: None,
            paths,
            saved_settings: settings.clone(),
            settings,
            files: Vec::new(),
            known_paths: HashSet::new(),
            view: View::Main,
            status: None,
            logical_cores,
            ffmpeg: FfmpegStatus::Searching,
            pending_probes: Vec::new(),
            ffmpeg_relocate_requested: false,
            next_file_id: 1,
        }
    }

    pub fn take_pending_probes(&mut self) -> Vec<(u64, PathBuf)> {
        std::mem::take(&mut self.pending_probes)
    }

    pub fn apply_probe_result(&mut self, id: u64, result: Result<MediaInfo, String>) {
        if let Some(file) = self.files.iter_mut().find(|f| f.id == id) {
            file.probe = match result {
                Ok(info) => ProbeState::Ready(info),
                Err(msg) => ProbeState::Failed(msg),
            };
        }
    }

    /// FFmpeg 탐색 결과 반영. 새로 찾은 경우 검사 실패/대기 파일을 다시 검사한다.
    pub fn apply_ffmpeg_status(&mut self, result: Result<String, String>) {
        match result {
            Ok(version) => {
                let was_missing = !matches!(self.ffmpeg, FfmpegStatus::Found { .. });
                self.ffmpeg = FfmpegStatus::Found { version };
                if was_missing {
                    for file in &mut self.files {
                        if !matches!(file.probe, ProbeState::Ready(_)) {
                            file.probe = ProbeState::Pending;
                            self.pending_probes.push((file.id, file.path.clone()));
                        }
                    }
                }
            }
            Err(msg) => {
                self.set_status(StatusLevel::Error, msg.clone());
                self.ffmpeg = FfmpegStatus::Missing(msg);
            }
        }
    }

    // ---------- 모델 ----------

    pub fn model_file_state(&self, model: WhisperModel) -> ModelFileState {
        self.model_files[model_index(model)]
    }

    pub fn refresh_model_state(&mut self, model: WhisperModel) {
        self.model_files[model_index(model)] = model::quick_check(&self.paths.models_dir, model);
    }

    pub fn model_op(&self, model: WhisperModel) -> Option<&ModelOp> {
        self.model_ops.iter().find(|op| op.model == model)
    }

    pub fn apply_model_event(&mut self, event: ModelEvent) {
        match event {
            ModelEvent::Progress { model, done, total } => {
                if let Some(op) = self.model_ops.iter_mut().find(|op| op.model == model) {
                    op.done = done;
                    op.total = total;
                }
            }
            ModelEvent::DownloadFinished { model, result } => {
                self.model_ops.retain(|op| op.model != model);
                self.refresh_model_state(model);
                match result {
                    Ok(()) => {
                        self.verified_models.push(model);
                        self.set_status(
                            StatusLevel::Info,
                            format!("{} 모델 다운로드 및 무결성 검증 완료", model.label()),
                        );
                    }
                    Err(msg) => self.set_status(
                        StatusLevel::Error,
                        format!("{} 모델 다운로드 실패: {msg}", model.label()),
                    ),
                }
            }
            ModelEvent::VerifyFinished { model, result } => {
                self.model_ops.retain(|op| op.model != model);
                match result {
                    Ok(()) => {
                        self.verified_models.push(model);
                        self.set_status(StatusLevel::Info, format!("{} 모델 무결성 확인 (SHA-256 일치)", model.label()));
                    }
                    Err(msg) => {
                        self.verified_models.retain(|m| *m != model);
                        self.set_status(StatusLevel::Error, format!("{} 모델 검증 실패: {msg}", model.label()));
                    }
                }
            }
        }
    }

    // ---------- 변환 작업 ----------

    // ---------- Auto 추천 ----------

    /// 변환 대상(시작 가능한 파일)의 (총 길이, 가장 긴 길이)
    fn pending_audio(&self) -> (Duration, Duration) {
        self.files
            .iter()
            .filter(|f| Self::is_startable(f))
            .filter_map(|f| match &f.probe {
                ProbeState::Ready(info) => info.duration,
                _ => None,
            })
            .fold((Duration::ZERO, Duration::ZERO), |(total, longest), d| (total + d, longest.max(d)))
    }

    /// 현재 PC/파일/설정 기준 추천. 하드웨어 정보 수집 전이면 None.
    pub fn recommendation(&self) -> Option<Recommendation> {
        let profile = self.system.as_ref()?;
        let (total_audio, longest_audio) = self.pending_audio();
        let provider = self.settings.cloud.provider;
        Some(recommend(&SelectorInput {
            profile,
            benchmarks: &self.settings.benchmarks,
            preferred_model: self.settings.whisper_model,
            is_installed: &|m| self.model_file_state(m) == ModelFileState::Installed,
            model_size: &|m| model::spec(m).size_bytes,
            total_audio,
            longest_audio,
            cloud_ready: self.key_slot(provider).key.is_some(),
            cloud_name: provider.label(),
        }))
    }

    /// 이번 실행에서 Cloud 를 쓰는지. Auto 는 추천 결과를 따른다.
    pub fn uses_cloud(&self) -> bool {
        match self.settings.engine_mode {
            EngineMode::Cloud => true,
            EngineMode::Local => false,
            EngineMode::Auto => matches!(self.recommendation().map(|r| r.choice), Some(Choice::Cloud)),
        }
    }

    /// 로컬 처리에 쓸 모델. Auto 는 메모리에 맞는 모델로 낮출 수 있다.
    pub fn local_model(&self) -> WhisperModel {
        if self.settings.engine_mode == EngineMode::Auto
            && let Some(Choice::Local { model }) = self.recommendation().map(|r| r.choice)
        {
            return model;
        }
        self.settings.whisper_model
    }

    pub fn engine_label(&self) -> String {
        let prefix = if self.settings.engine_mode == EngineMode::Auto { "Auto→" } else { "" };
        if self.uses_cloud() {
            let cloud = &self.settings.cloud;
            format!("{prefix}{} · {}", cloud.provider.label(), cloud.active().model)
        } else {
            format!("{prefix}Local · {}", self.local_model().label())
        }
    }

    /// 성능 측정에 쓸 실제 음성 샘플 (등록된 첫 번째 유효 파일).
    pub fn benchmark_sample(&self) -> Option<PathBuf> {
        self.files.iter().find(|f| matches!(f.probe, ProbeState::Ready(_))).map(|f| f.path.clone())
    }

    /// 변환 가능한 상태인지. 불가하면 사유를 돌려준다.
    pub fn start_blocker(&self) -> Option<String> {
        if !matches!(self.ffmpeg, FfmpegStatus::Found { .. }) {
            return Some("FFmpeg 를 찾을 수 없습니다.".to_owned());
        }
        if self.benchmark_running.is_some() {
            return Some("성능 측정이 끝난 뒤 시작할 수 있습니다.".to_owned());
        }
        // 검사 중인 파일이 있으면 일부만 시작되는 혼란을 막기 위해 기다린다.
        let probing = self.files.iter().filter(|f| matches!(f.probe, ProbeState::Pending)).count();
        if probing > 0 {
            return Some(format!("파일 검사가 끝난 뒤 시작할 수 있습니다 ({probing}개 검사 중)."));
        }
        if self.settings.engine_mode == EngineMode::Auto {
            match self.recommendation() {
                None => return Some("PC 성능 정보를 확인하는 중입니다.".to_owned()),
                Some(r) if r.choice == Choice::Unavailable => return Some(r.reasons.join("\n")),
                Some(_) => {}
            }
        }
        if self.uses_cloud() {
            let provider = self.settings.cloud.provider;
            if self.key_slot(provider).key.is_none() {
                return Some(format!("Cloud 모드에는 {} API Key 가 필요합니다. 설정에서 입력하세요.", provider.label()));
            }
            if self.settings.cloud.active().model.trim().is_empty() {
                return Some("Cloud 모델명이 비어 있습니다. 설정을 확인하세요.".to_owned());
            }
        } else {
            let model = self.local_model();
            if self.model_op(model).is_some() {
                return Some(format!("{} 모델 다운로드/검증이 진행 중입니다.", model.label()));
            }
            if self.model_file_state(model) != ModelFileState::Installed {
                return Some(format!("{} 모델이 설치되어 있지 않습니다. 설정에서 다운로드하세요.", model.label()));
            }
        }
        if !self.files.iter().any(Self::is_startable) {
            return Some(if self.has_active_jobs() {
                "변환이 진행 중입니다. 파일을 더 추가하면 이어서 시작할 수 있습니다.".to_owned()
            } else {
                "변환할 파일이 없습니다. 파일을 추가하세요.".to_owned()
            });
        }
        None
    }

    fn is_startable(file: &InputFile) -> bool {
        matches!(file.probe, ProbeState::Ready(_))
            && matches!(file.job, JobState::Idle | JobState::Cancelled | JobState::Failed(_))
    }

    /// Cloud 전송 동의 화면에 보여줄 정보: (파일 수, 총 길이)
    pub fn startable_summary(&self) -> (usize, Duration) {
        self.files
            .iter()
            .filter(|f| Self::is_startable(f))
            .fold((0, Duration::ZERO), |(n, d), f| match &f.probe {
                ProbeState::Ready(info) => (n + 1, d + info.duration.unwrap_or_default()),
                _ => (n, d),
            })
    }

    /// 대기/취소/실패 상태의 유효한 파일을 작업으로 만든다.
    /// Cloud 작업은 호출 전에 반드시 사용자 동의를 받아야 한다.
    pub fn build_jobs(&mut self) -> Vec<TranscribeJob> {
        let use_cloud = self.uses_cloud();
        let model = self.local_model();
        let model_path = model::model_path(&self.paths.models_dir, model);
        let cloud_settings = self.settings.cloud.clone();
        let api_key = self.key_slot(cloud_settings.provider).key.clone();
        let output_dir = self.output_dir().to_path_buf();
        let language = self.settings.language;
        let ffmpeg_dir = self.settings.ffmpeg_dir.clone();
        let engine_label = self.engine_label();

        // 진행 중인 작업이 없으면 새 일괄 처리로 통계를 시작한다 (진행 중이면 이어서 집계)
        if !self.has_active_jobs() {
            self.batch = Some(BatchStats::default());
        }
        let mut jobs = Vec::new();
        for file in self.files.iter_mut().filter(|f| Self::is_startable(f)) {
            let ProbeState::Ready(info) = &file.probe else { continue };
            let engine = if use_cloud {
                let Some(api_key) = api_key.clone() else { continue };
                JobEngine::Cloud { settings: cloud_settings.clone(), api_key }
            } else {
                JobEngine::Local { model, model_path: model_path.clone() }
            };
            let cancel = Arc::new(AtomicBool::new(false));
            jobs.push(TranscribeJob {
                id: file.id,
                input: file.path.clone(),
                duration: info.duration,
                output_dir: output_dir.clone(),
                engine,
                language,
                ffmpeg_dir: ffmpeg_dir.clone(),
                cancel: Arc::clone(&cancel),
            });
            file.engine_label = Some(engine_label.clone());
            file.job = JobState::Queued { cancel };
        }
        jobs
    }

    // ---------- API Key ----------

    pub fn key_slot(&self, provider: CloudProvider) -> &KeySlot {
        &self.keys[provider.index()]
    }

    pub fn key_slot_mut(&mut self, provider: CloudProvider) -> &mut KeySlot {
        &mut self.keys[provider.index()]
    }

    /// 입력란의 키를 지정한 공급자에 적용한다. `remember` 이면 Windows 자격 증명 관리자에 저장한다.
    pub fn apply_api_key_input(&mut self, provider: CloudProvider, remember: bool) {
        let mut input = std::mem::take(&mut self.api_key_input);
        let key = ApiKey::new(&input);
        input.zeroize();
        let Some(key) = key else {
            self.set_status(StatusLevel::Warning, "API Key 를 입력하세요.");
            return;
        };
        let name = provider.label();
        if remember {
            match secret::save_api_key(provider, &key) {
                Ok(()) => {
                    self.key_slot_mut(provider).saved = true;
                    self.set_status(
                        StatusLevel::Info,
                        format!("{name} API Key 를 적용하고 Windows 자격 증명 관리자에 저장했습니다."),
                    );
                }
                Err(err) => self.set_status(StatusLevel::Error, format!("API Key 저장 실패: {err}")),
            }
        } else {
            self.set_status(StatusLevel::Info, format!("{name} API Key 를 이번 실행에만 사용합니다 (저장하지 않음)."));
        }
        tracing::info!(provider = name, remember, "API Key 적용");
        let slot = self.key_slot_mut(provider);
        slot.key = Some(key);
        slot.check = KeyCheck::Unchecked;
        self.key_check_requested = Some(provider);
    }

    /// 메모리의 키를 지우고, 저장된 키가 있으면 자격 증명 관리자에서도 삭제한다.
    pub fn forget_api_key(&mut self, provider: CloudProvider) {
        let slot = self.key_slot_mut(provider);
        slot.key = None;
        slot.check = KeyCheck::Unchecked;
        match secret::delete_api_key(provider) {
            Ok(()) => {
                self.key_slot_mut(provider).saved = false;
                self.set_status(
                    StatusLevel::Info,
                    format!("{} API Key 를 삭제했습니다 (메모리 및 자격 증명 관리자).", provider.label()),
                );
            }
            Err(err) => self.set_status(StatusLevel::Error, format!("저장된 API Key 삭제 실패: {err}")),
        }
        tracing::info!(provider = provider.label(), "API Key 삭제");
    }

    pub fn apply_task_event(&mut self, event: TaskEvent) {
        let id = match &event {
            TaskEvent::Progress { id, .. }
            | TaskEvent::Completed { id, .. }
            | TaskEvent::Failed { id, .. }
            | TaskEvent::Cancelled { id } => *id,
            TaskEvent::BenchmarkDone { model, result } => {
                let model = *model;
                self.benchmark_running = None;
                match result {
                    Ok(b) => {
                        let msg = format!(
                            "{} 모델 성능 측정 완료: 약 {:.1}배속{}",
                            model.label(),
                            b.realtime_factor,
                            if b.real_audio { "" } else { " (합성 신호 사용, 정확도 낮음)" }
                        );
                        self.settings.benchmarks.retain(|x| x.model != model);
                        self.settings.benchmarks.push(b.clone());
                        self.set_status(StatusLevel::Info, msg);
                    }
                    Err(e) => self.set_status(StatusLevel::Error, format!("성능 측정 실패: {e}")),
                }
                return;
            }
        };
        let Some(file) = self.files.iter_mut().find(|f| f.id == id) else { return };
        match event {
            TaskEvent::Progress { stage, progress, .. } => {
                // 취소 요청 후 늦게 도착한 진행률은 무시하지 않고 갱신하되, 플래그는 유지한다.
                if let Some(cancel) = file.job.cancel_flag().cloned() {
                    file.job = JobState::Running { cancel, stage, progress };
                }
            }
            TaskEvent::Completed { output, elapsed, .. } => {
                let name = file.display_name.clone();
                self.results.push(ResultEntry {
                    source_name: name.clone(),
                    output: output.clone(),
                    engine: file.engine_label.clone().unwrap_or_default(),
                    elapsed,
                });
                file.job = JobState::Completed { output, elapsed };
                if let Some(b) = self.batch.as_mut() {
                    b.done += 1;
                }
                self.set_status(StatusLevel::Info, format!("완료: {name}"));
            }
            TaskEvent::Failed { error, .. } => {
                let name = file.display_name.clone();
                file.job = JobState::Failed(error);
                if let Some(b) = self.batch.as_mut() {
                    b.failed += 1;
                }
                self.set_status(StatusLevel::Error, format!("실패: {name}"));
            }
            TaskEvent::Cancelled { .. } => {
                file.job = JobState::Cancelled;
                if let Some(b) = self.batch.as_mut() {
                    b.cancelled += 1;
                }
            }
            TaskEvent::BenchmarkDone { .. } => {}
        }
        // 일괄 처리가 모두 끝나면 요약
        if !self.has_active_jobs()
            && let Some(b) = self.batch.take()
        {
            let level = if b.failed > 0 { StatusLevel::Warning } else { StatusLevel::Info };
            tracing::info!(done = b.done, failed = b.failed, cancelled = b.cancelled, "일괄 처리 종료");
            self.set_status(
                level,
                format!("모든 작업이 끝났습니다 — 성공 {} · 실패 {} · 취소 {}", b.done, b.failed, b.cancelled),
            );
        }
    }

    pub fn cancel_job(&mut self, id: u64) {
        if let Some(file) = self.files.iter_mut().find(|f| f.id == id)
            && let Some(cancel) = file.job.cancel_flag()
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn cancel_all(&mut self) {
        for file in &self.files {
            if let Some(cancel) = file.job.cancel_flag() {
                cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn has_active_jobs(&self) -> bool {
        self.files.iter().any(|f| f.job.is_active())
    }

    pub fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.status = Some(StatusMessage { level, text: text.into() });
    }

    pub fn output_dir(&self) -> &Path {
        self.settings
            .output_dir
            .as_deref()
            .unwrap_or(&self.paths.default_output_dir)
    }

    /// 파일 목록에 추가한다. 디렉터리는 한 단계 하위의 지원 파일만 추가한다.
    /// 반환값: (추가된 개수, 건너뛴 개수)
    pub fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> (usize, usize) {
        let mut added = 0;
        let mut skipped = 0;
        for path in paths {
            if path.is_dir() {
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        let mut children: Vec<PathBuf> = entries
                            .filter_map(Result::ok)
                            .map(|e| e.path())
                            .filter(|p| p.is_file())
                            .collect();
                        children.sort();
                        for child in children {
                            if self.add_file(child) {
                                added += 1;
                            } else {
                                skipped += 1;
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!(dir = %path.display(), error = %err, "폴더 읽기 실패");
                        skipped += 1;
                    }
                }
            } else if self.add_file(path) {
                added += 1;
            } else {
                skipped += 1;
            }
        }
        (added, skipped)
    }

    /// 지원 확장자이고 중복이 아니면 추가한다.
    fn add_file(&mut self, path: PathBuf) -> bool {
        if !has_supported_extension(&path) || self.known_paths.contains(&path) {
            return false;
        }
        let size_bytes = match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => meta.len(),
            Ok(_) => return false,
            Err(err) => {
                tracing::warn!(file = %path.display(), error = %err, "파일 정보 읽기 실패");
                return false;
            }
        };
        let display_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let id = self.next_file_id;
        self.next_file_id += 1;
        self.known_paths.insert(path.clone());
        self.pending_probes.push((id, path.clone()));
        self.files.push(InputFile {
            id,
            path,
            display_name,
            size_bytes,
            probe: ProbeState::Pending,
            job: JobState::Idle,
            engine_label: None,
        });
        true
    }

    /// 목록에서 제거한다. 진행 중인 작업은 취소한다 (이미 저장된 결과 파일은 유지).
    pub fn remove_file(&mut self, index: usize) {
        if index < self.files.len() {
            let removed = self.files.remove(index);
            if let Some(cancel) = removed.job.cancel_flag() {
                cancel.store(true, Ordering::Relaxed);
            }
            self.known_paths.remove(&removed.path);
        }
    }

    /// 완료·실패(검사 실패 포함) 항목을 목록에서 지운다. 결과 목록과 저장된 파일은 유지.
    pub fn clear_finished(&mut self) {
        let before = self.files.len();
        let known = &mut self.known_paths;
        self.files.retain(|f| {
            let finished = matches!(f.job, JobState::Completed { .. } | JobState::Failed(_))
                || (matches!(f.probe, ProbeState::Failed(_)) && !f.job.is_active());
            if finished {
                known.remove(&f.path);
            }
            !finished
        });
        let removed = before - self.files.len();
        self.set_status(StatusLevel::Info, format!("{removed}개 항목을 목록에서 지웠습니다."));
    }

    pub fn clear_files(&mut self) {
        self.cancel_all();
        self.files.clear();
        self.known_paths.clear();
    }

    /// 설정이 바뀌었으면 저장한다. 매 프레임 호출되지만 비교 비용은 작다.
    pub fn persist_settings_if_changed(&mut self) {
        if self.settings == self.saved_settings {
            return;
        }
        match self.settings.save(&self.paths.settings_file) {
            Ok(()) => {
                tracing::debug!("설정 저장 완료");
                self.saved_settings = self.settings.clone();
            }
            Err(err) => {
                tracing::error!(error = %err, "설정 저장 실패");
                self.set_status(StatusLevel::Error, format!("설정 저장 실패: {err}"));
                // 실패한 값을 기준으로 삼아 매 프레임 재시도/오류 반복을 막는다.
                self.saved_settings = self.settings.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_filter_is_case_insensitive() {
        assert!(has_supported_extension(Path::new(r"C:\영상\강의 1.MP4")));
        assert!(has_supported_extension(Path::new("a.flac")));
        assert!(!has_supported_extension(Path::new("a.txt")));
        assert!(!has_supported_extension(Path::new("noext")));
    }

    #[test]
    fn add_paths_dedups_and_scans_folder() {
        let dir = std::env::temp_dir()
            .join(format!("media_transcriber_state_{}", std::process::id()))
            .join("한글 폴더");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("녹음 파일.mp3"), b"x").expect("write");
        std::fs::write(dir.join("memo.txt"), b"x").expect("write");

        let paths = AppPaths {
            settings_file: dir.join("settings.json"),
            log_dir: dir.join("logs"),
            models_dir: dir.join("models"),
            ffmpeg_dir: dir.join("ffmpeg"),
            default_output_dir: dir.join("out"),
        };
        let mut state = AppState::new(paths, Settings::default());

        let (added, skipped) = state.add_paths([dir.clone()]);
        assert_eq!((added, skipped), (1, 1));
        assert_eq!(state.files[0].display_name, "녹음 파일.mp3");

        // 중복 등록 방지
        let (added, _) = state.add_paths([dir.join("녹음 파일.mp3")]);
        assert_eq!(added, 0);

        state.remove_file(0);
        assert!(state.files.is_empty());
        let (added, _) = state.add_paths([dir.join("녹음 파일.mp3")]);
        assert_eq!(added, 1);

        // 추가된 파일은 모두 검사 대기열에 들어간다 (첫 등록 + 재등록)
        let pending = state.take_pending_probes();
        assert_eq!(pending.len(), 2);
        let last_id = state.files[0].id;
        state.apply_probe_result(last_id, Err("손상된 파일".to_owned()));
        assert!(matches!(state.files[0].probe, ProbeState::Failed(_)));

        let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
    }

    fn test_state(name: &str) -> (AppState, PathBuf) {
        let dir = std::env::temp_dir().join(format!("media_transcriber_state_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let paths = AppPaths {
            settings_file: dir.join("settings.json"),
            log_dir: dir.join("logs"),
            models_dir: dir.join("models"),
            ffmpeg_dir: dir.join("ffmpeg"),
            default_output_dir: dir.join("out"),
        };
        (AppState::new(paths, Settings::default()), dir)
    }

    fn ready_info() -> MediaInfo {
        MediaInfo {
            format_name: "mp3".to_owned(),
            duration: Some(Duration::from_secs(10)),
            has_video: false,
            audio: crate::media::AudioStreamInfo { codec: "mp3".to_owned(), sample_rate: Some(16000), channels: Some(1) },
        }
    }

    #[test]
    fn batch_summary_and_clear_finished() {
        let (mut state, dir) = test_state("batch");
        for i in 0..3 {
            std::fs::write(dir.join(format!("파일 {i}.mp3")), b"x").expect("write");
        }
        state.add_paths([dir.clone()]);
        let ids: Vec<u64> = state.files.iter().map(|f| f.id).collect();
        state.ffmpeg = FfmpegStatus::Found { version: "test".to_owned() };
        assert!(state.start_blocker().is_some_and(|r| r.contains("3개 검사 중")));
        for id in &ids {
            state.apply_probe_result(*id, Ok(ready_info()));
        }
        // 모델이 없으니 Local 로 job 을 직접 만든다
        let jobs = state.build_jobs();
        assert_eq!(jobs.len(), 3);
        assert!(state.has_active_jobs());

        state.apply_task_event(TaskEvent::Completed { id: ids[0], output: dir.join("a.txt"), elapsed: Duration::from_secs(1) });
        state.apply_task_event(TaskEvent::Failed { id: ids[1], error: "오류".to_owned() });
        assert!(state.has_active_jobs());
        state.apply_task_event(TaskEvent::Cancelled { id: ids[2] });
        assert!(!state.has_active_jobs());
        let msg = state.status.as_ref().map(|s| s.text.clone()).unwrap_or_default();
        assert!(msg.contains("성공 1") && msg.contains("실패 1") && msg.contains("취소 1"), "{msg}");
        assert_eq!(state.results.len(), 1);

        // 완료·실패만 지우고, 취소 항목과 결과 목록은 남긴다
        state.clear_finished();
        assert_eq!(state.files.len(), 1);
        assert!(matches!(state.files[0].job, JobState::Cancelled));
        assert_eq!(state.results.len(), 1);
        // 지운 파일은 다시 등록할 수 있다
        let (added, _) = state.add_paths([dir.join("파일 0.mp3")]);
        assert_eq!(added, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
