//! Local 작업 스레드 (whisper.cpp).
//!
//! - 단일 스레드에서 순차 처리한다 (Local 동시 실행 1개 — 추론이 이미 CPU 코어를 모두 쓴다).
//! - Whisper 모델은 한 번 로드해 재사용하고, 다른 모델이 요청될 때만 교체한다.
//! - 긴 파일은 구간별로 디코딩·인식해 메모리 사용을 제한한다 (앞 구간 끝을 문맥으로 전달).
//! - 성능 측정도 같은 스레드에서 처리해 변환과 겹치지 않게 한다.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use super::job::{
    BenchmarkJob, DECODE_SPAN, JobEngine, JobError, Notify, Stage, TRANSCRIBE_END, TaskEvent, TranscribeJob,
    finish_event,
};
use crate::config::{Language, WhisperModel};
use crate::engine_selector::BenchmarkResult;
use crate::media::{FfmpegTools, WHISPER_SAMPLE_RATE, audio};
use crate::output::txt;
use crate::speech::Transcript;
use crate::speech::local::{LocalWhisper, default_thread_count};

/// 이 길이보다 긴 파일은 구간별로 처리한다.
pub const LOCAL_CHUNK_THRESHOLD: Duration = Duration::from_secs(20 * 60);
/// 구간 길이 (16 kHz f32 기준 10분 ≈ 38 MB).
pub const LOCAL_CHUNK: Duration = Duration::from_secs(10 * 60);
/// 다음 구간에 문맥으로 넘길 앞 구간 끝부분 글자 수.
const PROMPT_CHARS: usize = 200;

/// 진행률 보고 함수 (whisper 콜백에 넘기기 위해 'static + 공유 가능)
type Progress = Arc<dyn Fn(Stage, f32) + Send + Sync>;

pub(super) enum LocalRequest {
    Transcribe(Box<TranscribeJob>),
    Benchmark(BenchmarkJob),
}

struct WorkerState {
    whisper: Option<LocalWhisper>,
    ffmpeg: Option<(Option<PathBuf>, FfmpegTools)>,
}

impl WorkerState {
    fn tools(&mut self, dir: &Option<PathBuf>) -> Result<FfmpegTools, JobError> {
        match &self.ffmpeg {
            Some((cached, tools)) if cached == dir => Ok(tools.clone()),
            _ => {
                let tools = FfmpegTools::locate(dir.as_deref())?;
                self.ffmpeg = Some((dir.clone(), tools.clone()));
                Ok(tools)
            }
        }
    }

    /// 같은 모델이면 재사용, 다르면 이전 모델을 먼저 해제하고 로드한다.
    fn whisper(&mut self, model: WhisperModel, path: &Path) -> Result<&LocalWhisper, JobError> {
        if self.whisper.as_ref().map(LocalWhisper::model) != Some(model) {
            self.whisper = None;
            self.whisper = Some(LocalWhisper::load(path, model)?);
        }
        self.whisper.as_ref().ok_or_else(|| JobError::Failed("모델이 로드되지 않았습니다".to_owned()))
    }
}

pub(super) fn run(rx: Receiver<LocalRequest>, tx: Sender<TaskEvent>, notify: Notify) {
    let mut state = WorkerState { whisper: None, ffmpeg: None };
    for request in rx {
        let event = match request {
            LocalRequest::Transcribe(job) => {
                let started = Instant::now();
                let result = process(&mut state, &job, &tx, &notify);
                finish_event(&job, result, started.elapsed())
            }
            LocalRequest::Benchmark(bench) => {
                let model = bench.model;
                let result = benchmark(&mut state, &bench).map_err(|e| match e {
                    JobError::Cancelled => "취소됨".to_owned(),
                    JobError::Failed(msg) => msg,
                });
                match &result {
                    Ok(b) => tracing::info!(model = model.label(), rtf = b.realtime_factor, real_audio = b.real_audio, "성능 측정 완료"),
                    Err(e) => tracing::warn!(model = model.label(), error = %e, "성능 측정 실패"),
                }
                TaskEvent::BenchmarkDone { model, result }
            }
        };
        if tx.send(event).is_err() {
            break; // GUI 종료
        }
        notify();
    }
    tracing::debug!("Local 작업 스레드 종료");
}

fn process(
    state: &mut WorkerState,
    job: &TranscribeJob,
    tx: &Sender<TaskEvent>,
    notify: &Notify,
) -> Result<PathBuf, JobError> {
    let JobEngine::Local { model, model_path } = &job.engine else {
        return Err(JobError::Failed("Local 작업이 아닙니다".to_owned()));
    };
    let id = job.id;
    let (ptx, pnotify) = (tx.clone(), Arc::clone(notify));
    let send: Progress = Arc::new(move |stage: Stage, progress: f32| {
        let _ = ptx.send(TaskEvent::Progress { id, stage, progress });
        pnotify();
    });
    check_cancel(&job.cancel)?;
    let tools = state.tools(&job.ffmpeg_dir)?;

    // URL 작업이면 오디오를 임시 WAV 로 받는다 (input 이 사라질 때 자동 삭제)
    let input = super::input::prepare(job, &tools, &*send)?;
    check_cancel(&job.cancel)?;
    // 이후 단계의 진행률은 URL 작업이면 40~100% 로 옮긴다
    let raw = Arc::clone(&send);
    let is_url = job.url.is_some();
    let send: Progress = Arc::new(move |stage: Stage, p: f32| {
        raw(stage, if is_url { super::input::remap_url(p) } else { p });
    });

    if state.whisper.as_ref().map(LocalWhisper::model) != Some(*model) {
        send(Stage::LoadingModel, 0.0);
    }
    let whisper = state.whisper(*model, model_path)?;
    check_cancel(&job.cancel)?;

    let transcript = transcribe_file(whisper, &tools, job, input.path(), input.duration, LOCAL_CHUNK_THRESHOLD, LOCAL_CHUNK, &send)?;
    check_cancel(&job.cancel)?;

    send(Stage::Saving, TRANSCRIBE_END);
    Ok(txt::save_txt_named(&job.output_dir, &input.output_stem, &transcript)?)
}

/// 파일 하나를 인식한다. `threshold` 보다 길면 `chunk` 길이로 나눠 처리한다.
#[allow(clippy::too_many_arguments)]
fn transcribe_file(
    whisper: &LocalWhisper,
    tools: &FfmpegTools,
    job: &TranscribeJob,
    input: &Path,
    duration: Option<Duration>,
    threshold: Duration,
    chunk: Duration,
    send: &Progress,
) -> Result<Transcript, JobError> {
    let ranges: Vec<(Duration, Option<Duration>)> = match duration {
        Some(total) if total > threshold => {
            let mut v = Vec::new();
            let mut start = Duration::ZERO;
            while start < total {
                v.push((start, Some(chunk.min(total - start))));
                start += chunk;
            }
            v
        }
        // 짧거나 길이를 모르면 한 번에 처리
        _ => vec![(Duration::ZERO, None)],
    };
    let n = ranges.len() as f32;
    let span = TRANSCRIBE_END / n;
    let mut merged = Transcript::default();
    let mut prompt = String::new();

    for (i, (start, len)) in ranges.iter().enumerate() {
        let base = i as f32 * span;
        check_cancel(&job.cancel)?;

        // 오디오 추출 (구간 또는 전체)
        send(Stage::Decoding, base);
        let pcm = match len {
            Some(len) => audio::decode_range_pcm(tools, input, *start, *len, &job.cancel)?,
            None => {
                let mut last = -1.0f32;
                audio::decode_to_whisper_pcm(tools, input, duration, &job.cancel, |p| {
                    if p - last >= 0.05 {
                        last = p;
                        send(Stage::Decoding, p * DECODE_SPAN);
                    }
                })?
                .samples
            }
        };
        check_cancel(&job.cancel)?;

        // 인식 (whisper 진행률 콜백은 'static 이어야 하므로 Arc 로 넘긴다)
        let decode_part = if n > 1.0 { span * 0.05 } else { DECODE_SPAN };
        send(Stage::Transcribing, base + decode_part);
        let live = Arc::clone(send);
        let mut part = whisper.transcribe(
            &pcm,
            job.language,
            default_thread_count(),
            Arc::clone(&job.cancel),
            (!prompt.is_empty()).then_some(prompt.as_str()),
            move |percent| {
                let p = base + decode_part + (span - decode_part) * (percent.clamp(0, 100) as f32 / 100.0);
                live(Stage::Transcribing, p);
            },
        )?;
        drop(pcm); // 다음 구간 전에 오디오 버퍼 해제

        for seg in &mut part.segments {
            seg.start += *start;
            seg.end += *start;
        }
        prompt = tail_chars(&part.plain_text(), PROMPT_CHARS);
        if merged.language.is_none() {
            merged.language = part.language.take();
        }
        merged.segments.append(&mut part.segments);
        send(Stage::Transcribing, base + span);
        if n > 1.0 {
            tracing::info!(id = job.id, chunk = i + 1, total = ranges.len(), "Local 구간 인식 완료");
        }
    }
    Ok(merged)
}

fn tail_chars(text: &str, n: usize) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    chars[chars.len().saturating_sub(n)..].iter().collect()
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), JobError> {
    if cancel.load(Ordering::Relaxed) { Err(JobError::Cancelled) } else { Ok(()) }
}

/// 측정 샘플 길이 (whisper 의 한 처리 창과 같은 30초)
const BENCH_SECONDS: u64 = 30;

/// 로컬 처리 속도 측정: 모델 로드 시간은 빼고, 30초 샘플 추론 시간만 잰다.
fn benchmark(state: &mut WorkerState, job: &BenchmarkJob) -> Result<BenchmarkResult, JobError> {
    let limit = Duration::from_secs(BENCH_SECONDS);
    let never = AtomicBool::new(false);

    // 실제 음성 샘플을 우선 사용하고, 실패하면 합성 신호로 측정한다.
    let real = match &job.sample {
        Some(path) => match state.tools(&job.ffmpeg_dir) {
            Ok(tools) => match audio::decode_range_pcm(&tools, path, Duration::ZERO, limit, &never) {
                Ok(pcm) if pcm.len() >= WHISPER_SAMPLE_RATE as usize * 5 => Some(pcm),
                Ok(_) => None, // 5초 미만은 측정에 부적합
                Err(err) => {
                    tracing::warn!(error = %err, "측정 샘플 디코딩 실패, 합성 신호 사용");
                    None
                }
            },
            Err(_) => None,
        },
        None => None,
    };
    let real_audio = real.is_some();
    // whisper 는 짧은 음성도 30초 창 단위로 인코딩하므로, 짧은 샘플은 반복해 30초를 채워야
    // 실제 긴 파일 처리 속도에 가까운 값이 나온다.
    let pcm = fill_to(real.unwrap_or_else(|| synthetic_signal(limit)), limit);

    let whisper = state.whisper(job.model, &job.model_path)?;
    let threads = default_thread_count();
    let started = Instant::now();
    whisper.transcribe(&pcm, Language::Auto, threads, Arc::new(AtomicBool::new(false)), None, |_| {})?;
    let elapsed = started.elapsed().as_secs_f32().max(0.001);
    let audio_secs = pcm.len() as f32 / WHISPER_SAMPLE_RATE as f32;

    let measured_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok(BenchmarkResult {
        model: job.model,
        realtime_factor: audio_secs / elapsed,
        threads: u32::try_from(threads).unwrap_or(u32::MAX),
        real_audio,
        measured_at,
    })
}

/// 샘플을 반복해 정확히 `length` 길이로 맞춘다.
fn fill_to(mut pcm: Vec<f32>, length: Duration) -> Vec<f32> {
    let target = WHISPER_SAMPLE_RATE as usize * length.as_secs() as usize;
    if pcm.is_empty() {
        return pcm;
    }
    let original = pcm.len();
    while pcm.len() < target {
        let take = original.min(target - pcm.len());
        pcm.extend_from_within(..take);
    }
    pcm.truncate(target);
    pcm
}

/// 음성 대역 잡음 + 음절 리듬 포락선의 합성 신호 (실제 음성 파일이 없을 때만 사용, 정확도 낮음).
fn synthetic_signal(length: Duration) -> Vec<f32> {
    let rate = WHISPER_SAMPLE_RATE as usize;
    let n = rate * length.as_secs() as usize;
    let mut seed: u32 = 0x9E37_79B9;
    let mut out = Vec::with_capacity(n);
    let mut prev = 0.0f32;
    for i in 0..n {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let white = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
        prev = 0.85 * prev + 0.15 * white; // 저역 통과 → 음성 대역에 가깝게
        let t = i as f32 / rate as f32;
        let envelope = (0.5 + 0.5 * (t * std::f32::consts::TAU * 4.0).sin()).powi(2); // 초당 약 4 음절
        out.push(prev * envelope * 0.6);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_sample_is_filled_to_window() {
        let rate = WHISPER_SAMPLE_RATE as usize;
        let short: Vec<f32> = (0..rate * 12).map(|i| i as f32).collect();
        let filled = fill_to(short, Duration::from_secs(30));
        assert_eq!(filled.len(), rate * 30);
        assert_eq!(filled[rate * 12], 0.0); // 처음부터 반복
        let long = vec![0.5f32; rate * 40];
        assert_eq!(fill_to(long, Duration::from_secs(30)).len(), rate * 30);
        assert!(fill_to(Vec::new(), Duration::from_secs(30)).is_empty());
        let synth = synthetic_signal(Duration::from_secs(30));
        assert_eq!(synth.len(), rate * 30);
        assert!(synth.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn tail_chars_keeps_end() {
        assert_eq!(tail_chars("  안녕하세요 반갑습니다  ", 5), "반갑습니다");
        assert_eq!(tail_chars("abc", 10), "abc");
    }

    /// 긴 파일 구간 처리: 구간 경계에서 문장을 잃지 않고, 타임스탬프가 구간 위치만큼 이동한다.
    /// (Base 모델과 Windows TTS 가 있을 때만 실행)
    #[test]
    fn long_file_is_processed_in_chunks() {
        let Some(models) = crate::config::AppPaths::resolve().map(|p| p.models_dir) else { return };
        let model_path = crate::speech::model::model_path(&models, WhisperModel::Base);
        if !model_path.is_file() {
            eprintln!("Base 모델 없음 - 건너뜀");
            return;
        }
        let Ok(tools) = FfmpegTools::locate(None) else { return };
        let dir = std::env::temp_dir()
            .join(format!("media_transcriber_chunk_{}", std::process::id()))
            .join("구간 테스트");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let wav = dir.join("long.wav");
        let script = r#"
            Add-Type -AssemblyName System.Speech
            $s = New-Object System.Speech.Synthesis.SpeechSynthesizer
            $v = $s.GetInstalledVoices() | Where-Object { $_.VoiceInfo.Culture.Name -eq 'en-US' } | Select-Object -First 1
            if (-not $v) { exit 3 }
            $s.SelectVoice($v.VoiceInfo.Name)
            $s.SetOutputToWaveFile($env:TTS_OUT)
            $s.Speak('The first chapter is about apples. The second chapter is about bananas. The third chapter is about cherries. The final chapter is about grapes.')
            $s.Dispose()
        "#;
        let ok = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("TTS_OUT", &wav)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("영어 TTS 없음 - 건너뜀");
            return;
        }
        let info = tools.probe(&wav).expect("probe");
        let total = info.duration.expect("duration");
        let whisper = LocalWhisper::load(&model_path, WhisperModel::Base).expect("load");
        let job = TranscribeJob {
            id: 1,
            input: wav.clone(),
            url: None,
            duration: Some(total),
            output_dir: dir.clone(),
            engine: JobEngine::Local { model: WhisperModel::Base, model_path: model_path.clone() },
            language: Language::English,
            ffmpeg_dir: None,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let progress = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&progress);
        let report: Progress = Arc::new(move |_, p| {
            if let Ok(mut v) = sink.lock() {
                v.push(p);
            }
        });
        // 구간 길이를 짧게 해 여러 구간으로 나눈다 (전체 약 10초 → 4초 구간)
        let t = transcribe_file(&whisper, &tools, &job, &wav, Some(total), Duration::from_secs(1), Duration::from_secs(4), &report)
            .expect("transcribe");
        let text = t.plain_text().to_lowercase();
        eprintln!("[chunked] {text}");
        assert!(text.contains("apple"), "{text}");
        assert!(text.contains("grape"), "{text}");
        // 뒤 구간의 타임스탬프는 구간 시작 위치 이후여야 한다
        assert!(t.segments.last().is_some_and(|s| s.start >= Duration::from_secs(4)));
        let p = progress.lock().map(|v| v.clone()).unwrap_or_default();
        assert!(p.windows(2).all(|w| w[1] >= w[0] - 1e-6), "진행률은 감소하지 않아야 함: {p:?}");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
    }
}
