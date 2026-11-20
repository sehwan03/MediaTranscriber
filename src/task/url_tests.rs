//! URL 작업 전체 흐름 테스트: 탐색한 영상 → 작업 관리자 → (오디오 추출 → 기존 Local/Cloud 인식 → TXT) → 임시 파일 삭제.
//!
//! 로컬 HTTP 서버로 영상을 제공하므로 외부 사이트에 접속하지 않는다.
//! yt-dlp · FFmpeg · (Local 은) Base 모델 · Windows TTS 가 없으면 건너뛴다.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{JobEngine, TaskEvent, TaskManager, TranscribeJob, UrlJob};
use crate::config::secret::ApiKey;
use crate::config::settings::{CloudSettings, ProviderSettings};
use crate::config::{AppPaths, CloudProvider, Language, WhisperModel};
use crate::url::tests::{fixture_dir, make_video, serve_dir, serve_dir_slow, ytdlp};
use crate::url::{UrlAuth, UrlVideo};

struct Env {
    dir: PathBuf,
    temp: PathBuf,
    out: PathBuf,
    ytdlp_dir: PathBuf,
}

fn env(name: &str) -> Option<Env> {
    ytdlp()?;
    crate::media::FfmpegTools::locate(None).ok()?;
    let dir = fixture_dir(&format!("job_{name}")); // url::tests 와 폴더가 겹치지 않게
    let temp = dir.parent()?.join("임시 폴더");
    let out = dir.parent()?.join("결과 폴더");
    std::fs::create_dir_all(&temp).ok()?;
    Some(Env { dir, temp, out, ytdlp_dir: AppPaths::resolve()?.ytdlp_dir })
}

fn temp_is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(true)
}

/// 영상 하나를 탐색해 돌려준다 (실제 사용 흐름과 같게 yt-dlp 탐색 결과를 쓴다)
fn discover_one(url: &str) -> UrlVideo {
    let y = ytdlp().expect("ytdlp");
    let mut videos = y.discover(url, &UrlAuth::None, &AtomicBool::new(false)).expect("discover");
    let tools = crate::media::FfmpegTools::locate(None).expect("ffmpeg");
    crate::url::fill_missing_durations(&mut videos, &tools, &AtomicBool::new(false));
    videos.remove(0)
}

fn url_job(id: u64, video: UrlVideo, env: &Env, engine: JobEngine, language: Language) -> (TranscribeJob, Arc<AtomicBool>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let job = TranscribeJob {
        id,
        input: PathBuf::from(video.key()),
        duration: video.duration,
        url: Some(UrlJob { video, auth: UrlAuth::None, ytdlp_dir: env.ytdlp_dir.clone(), temp_dir: env.temp.clone() }),
        output_dir: env.out.clone(),
        engine,
        language,
        ffmpeg_dir: None,
        cancel: Arc::clone(&cancel),
    };
    (job, cancel)
}

fn wait_finished(manager: &TaskManager, timeout: Duration) -> (TaskEvent, Vec<(super::Stage, f32)>) {
    let deadline = Instant::now() + timeout;
    let mut progress = Vec::new();
    while Instant::now() < deadline {
        match manager.try_recv() {
            Some(TaskEvent::Progress { stage, progress: p, .. }) => progress.push((stage, p)),
            Some(e @ (TaskEvent::Completed { .. } | TaskEvent::Failed { .. } | TaskEvent::Cancelled { .. })) => {
                return (e, progress);
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    panic!("시간 안에 끝나지 않음");
}

/// Windows TTS 로 영어 문장을 말하는 짧은 영상을 만든다. TTS 가 없으면 None.
fn speech_video(dir: &Path, name: &str) -> Option<PathBuf> {
    let wav = dir.join("speech.wav");
    let script = r#"
        Add-Type -AssemblyName System.Speech
        $s = New-Object System.Speech.Synthesis.SpeechSynthesizer
        $v = $s.GetInstalledVoices() | Where-Object { $_.VoiceInfo.Culture.Name -eq 'en-US' } | Select-Object -First 1
        if (-not $v) { exit 3 }
        $s.SelectVoice($v.VoiceInfo.Name)
        $s.SetOutputToWaveFile($env:TTS_OUT)
        $s.Speak('Welcome to the online lecture. Today we study the history of computers.')
        $s.Dispose()
    "#;
    let ok = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("TTS_OUT", &wav)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return None;
    }
    let out = dir.join(name);
    let ok = std::process::Command::new(crate::url::tests::ffmpeg_exe())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=gray:s=160x120:r=10", "-i"])
        .arg(&wav)
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", "-movflags", "+faststart"])
        .arg(&out)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(out)
}

#[test]
fn url_video_with_local_whisper_saves_txt_and_removes_temp() {
    let Some(env) = env("local") else { return };
    let model_path = crate::speech::model::model_path(&AppPaths::resolve().expect("paths").models_dir, WhisperModel::Base);
    if !model_path.is_file() {
        eprintln!("Base 모델 없음 - 건너뜀");
        return;
    }
    if speech_video(&env.dir, "lecture.mp4").is_none() {
        eprintln!("영어 TTS 없음 - 건너뜀");
        return;
    }
    let base = serve_dir(env.dir.clone());
    let mut video = discover_one(&format!("{base}/lecture.mp4"));
    video.title = "1주차: 컴퓨터의 역사".to_owned(); // 제목 기반 파일명 확인 (금지 문자 포함)

    let manager = TaskManager::spawn(|| {}).expect("spawn");
    let (job, _) = url_job(1, video, &env, JobEngine::Local { model: WhisperModel::Base, model_path }, Language::English);
    manager.submit(job).expect("submit");
    let (event, progress) = wait_finished(&manager, Duration::from_secs(120));
    let TaskEvent::Completed { output, .. } = event else { panic!("완료되어야 함") };

    assert_eq!(output.file_name().and_then(|n| n.to_str()), Some("1주차_ 컴퓨터의 역사.txt"));
    let text = std::fs::read_to_string(&output).expect("read").to_lowercase();
    eprintln!("[url-local] {text}");
    assert!(text.contains("lecture") && text.contains("computer"), "{text}");
    assert!(progress.iter().any(|(s, _)| *s == super::Stage::Downloading), "오디오 받기 단계가 있어야 함");
    let ps: Vec<f32> = progress.iter().map(|(_, p)| *p).collect();
    assert!(ps.windows(2).all(|w| w[1] >= w[0] - 1e-4), "진행률은 줄지 않아야 함: {ps:?}");
    assert!(temp_is_empty(&env.temp), "완료 후 임시 오디오가 지워져야 함");
    let _ = std::fs::remove_dir_all(env.dir.parent().unwrap_or(&env.dir));
}

#[test]
fn url_video_with_cloud_engine_uses_existing_upload_and_removes_temp() {
    let Some(env) = env("cloud") else { return };
    make_video(&env.dir.join("talk.mp4"), 4);
    let base = serve_dir(env.dir.clone());
    let video = discover_one(&format!("{base}/talk.mp4"));

    // 모의 OpenAI 서버 (기존 Cloud 경로 그대로)
    let api = crate::task::tests::text_server("클라우드 결과");
    let settings = CloudSettings {
        provider: CloudProvider::OpenAi,
        openai: ProviderSettings { base_url: api.clone(), model: "gpt-transcribe".to_owned() },
        ..CloudSettings::default()
    };
    let engine = JobEngine::Cloud { settings, api_key: ApiKey::new("fake-url-cloud-key-000").expect("key") };
    let manager = TaskManager::spawn(|| {}).expect("spawn");
    let (job, _) = url_job(2, video, &env, engine, Language::Korean);
    manager.submit(job).expect("submit");
    let (event, _) = wait_finished(&manager, Duration::from_secs(60));
    let TaskEvent::Completed { output, .. } = event else { panic!("완료되어야 함: Cloud") };
    assert_eq!(std::fs::read_to_string(&output).expect("read"), "클라우드 결과\n");
    assert!(temp_is_empty(&env.temp), "완료 후 임시 오디오가 지워져야 함");
    let _ = std::fs::remove_dir_all(env.dir.parent().unwrap_or(&env.dir));
}

#[test]
fn cancelling_url_download_removes_temp() {
    let Some(env) = env("cancel") else { return };
    make_video(&env.dir.join("long.mp4"), 60);
    let fast = serve_dir(env.dir.clone());
    let mut video = discover_one(&format!("{fast}/long.mp4"));
    // 실제 받기는 느린 서버에서 (도중에 취소하기 위해)
    let slow = serve_dir_slow(env.dir.clone(), Duration::from_millis(100));
    video.source_url = format!("{slow}/long.mp4");

    let model_path = PathBuf::from("unused.bin");
    let manager = TaskManager::spawn(|| {}).expect("spawn");
    let (job, cancel) = url_job(3, video, &env, JobEngine::Local { model: WhisperModel::Tiny, model_path }, Language::Auto);
    manager.submit(job).expect("submit");
    // 받기가 시작되면 취소
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && temp_is_empty(&env.temp) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!temp_is_empty(&env.temp), "받는 중에는 임시 파일이 있어야 함");
    cancel.store(true, Ordering::Relaxed);
    let (event, _) = wait_finished(&manager, Duration::from_secs(20));
    assert!(matches!(event, TaskEvent::Cancelled { id: 3 }), "취소되어야 함");
    std::thread::sleep(Duration::from_millis(300));
    assert!(temp_is_empty(&env.temp), "취소 후 임시 오디오가 지워져야 함");
    let _ = std::fs::remove_dir_all(env.dir.parent().unwrap_or(&env.dir));
}

#[test]
fn failed_url_download_reports_error_and_removes_temp() {
    let Some(env) = env("fail") else { return };
    make_video(&env.dir.join("ok.mp4"), 2);
    let base = serve_dir(env.dir.clone());
    let mut video = discover_one(&format!("{base}/ok.mp4"));
    std::fs::remove_file(env.dir.join("ok.mp4")).expect("remove"); // 탐색 후 사라진 영상
    video.source_url = format!("{base}/ok.mp4?sig=SECRET");

    let manager = TaskManager::spawn(|| {}).expect("spawn");
    let (job, _) = url_job(4, video, &env, JobEngine::Local { model: WhisperModel::Tiny, model_path: PathBuf::from("x") }, Language::Auto);
    manager.submit(job).expect("submit");
    let (event, _) = wait_finished(&manager, Duration::from_secs(30));
    let TaskEvent::Failed { error, .. } = event else { panic!("실패해야 함") };
    eprintln!("[url-fail] {error}");
    assert!(!error.contains("SECRET"), "{error}");
    assert!(temp_is_empty(&env.temp), "실패 후 임시 오디오가 지워져야 함");
    let _ = std::fs::remove_dir_all(env.dir.parent().unwrap_or(&env.dir));
}
