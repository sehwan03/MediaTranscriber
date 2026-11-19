//! 로컬 Whisper 통합 테스트.
//!
//! - 모델: `%LOCALAPPDATA%\MediaTranscriber\data\models` 의 Base 모델을 사용한다.
//!   없으면 테스트를 건너뛴다. 모델 다운로드 자체는 `download_base_model` (ignored) 로 검증한다:
//!   `cargo test download_base_model -- --ignored`
//! - 음성: Windows 내장 TTS(System.Speech)로 한국어/영어 문장을 WAV 로 만든 뒤
//!   MP3/MP4 로 변환해 전체 파이프라인(추출 → 인식 → TXT 저장)을 검증한다.
//! - 인식 단계는 네트워크를 사용하지 않는다.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::local::{LocalWhisper, default_thread_count};
use super::model::{self, ModelFileState};
use crate::config::{AppPaths, Language, WhisperModel};
use crate::media::{FfmpegTools, audio};
use crate::output::txt;

const MODEL: WhisperModel = WhisperModel::Base;

fn models_dir() -> Option<PathBuf> {
    AppPaths::resolve().map(|p| p.models_dir)
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("media_transcriber_speech_{name}_{}", std::process::id()))
        .join("음성 테스트");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// Windows TTS 로 WAV 생성. 음성이 설치되어 있지 않으면 None.
fn tts(text: &str, culture: &str, out: &Path) -> Option<()> {
    let script = r#"
        Add-Type -AssemblyName System.Speech
        $s = New-Object System.Speech.Synthesis.SpeechSynthesizer
        $v = $s.GetInstalledVoices() | Where-Object { $_.VoiceInfo.Culture.Name -eq $env:TTS_CULTURE } | Select-Object -First 1
        if (-not $v) { exit 3 }
        $s.SelectVoice($v.VoiceInfo.Name)
        $s.Rate = -1
        $s.SetOutputToWaveFile($env:TTS_OUT)
        $s.Speak($env:TTS_TEXT)
        $s.Dispose()
    "#;
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("TTS_TEXT", text)
        .env("TTS_CULTURE", culture)
        .env("TTS_OUT", out)
        .status()
        .ok()?;
    (status.success() && out.is_file()).then_some(())
}

fn ffmpeg_exe() -> PathBuf {
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join(r"Microsoft\WinGet\Links"));
    }
    dirs.into_iter().map(|d| d.join("ffmpeg.exe")).find(|p| p.is_file()).expect("ffmpeg.exe")
}

fn convert(input: &Path, output: &Path, extra: &[&str]) {
    let status = Command::new(ffmpeg_exe())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(extra)
        .arg(output)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
}

/// 전체 파이프라인 실행 후 저장된 TXT 내용을 돌려준다.
fn run_pipeline(
    whisper: &LocalWhisper,
    tools: &FfmpegTools,
    input: &Path,
    out_dir: &Path,
    lang: Language,
) -> (String, Option<String>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let info = tools.probe(input).expect("probe");
    let pcm = audio::decode_to_whisper_pcm(tools, input, info.duration, &cancel, |_| {}).expect("decode");
    let started = std::time::Instant::now();
    let transcript = whisper
        .transcribe(&pcm.samples, lang, default_thread_count(), cancel, None, |_| {})
        .expect("transcribe");
    eprintln!(
        "인식 시간 {:.2}초 (음성 {:.1}초)",
        started.elapsed().as_secs_f32(),
        pcm.samples.len() as f32 / 16_000.0
    );
    let saved = txt::save_txt(out_dir, input, &transcript).expect("save");
    (std::fs::read_to_string(saved).expect("read"), transcript.language)
}

struct Env {
    whisper: LocalWhisper,
    tools: FfmpegTools,
}

fn env() -> Option<Env> {
    let dir = models_dir()?;
    if model::quick_check(&dir, MODEL) != ModelFileState::Installed {
        eprintln!("{} 모델 없음 - 테스트 건너뜀 (cargo test download_base_model -- --ignored)", MODEL.label());
        return None;
    }
    let tools = FfmpegTools::locate(None).ok()?;
    let whisper = LocalWhisper::load(&model::model_path(&dir, MODEL), MODEL).expect("모델 로드");
    Some(Env { whisper, tools })
}

#[test]
#[ignore = "네트워크로 약 148MB 모델을 내려받는다"]
fn download_base_model() {
    let dir = models_dir().expect("dir");
    let mut last = 0;
    let path = model::download(&dir, MODEL, &AtomicBool::new(false), |done, total| {
        let pct = done * 100 / total.max(1);
        if pct >= last + 20 {
            last = pct;
            eprintln!("다운로드 {pct}%");
        }
    })
    .expect("download");
    assert_eq!(model::quick_check(&dir, MODEL), ModelFileState::Installed);
    model::verify(&dir, MODEL, &AtomicBool::new(false), |_, _| {}).expect("verify");
    eprintln!("저장: {}", path.display());
}

#[test]
fn transcribes_korean_and_english_offline() {
    let Some(env) = env() else { return };
    let dir = temp_dir("ko_en");

    // 한국어: MP3 (공백 포함 한글 파일명)
    let ko_wav = dir.join("한국어 원본.wav");
    if tts("안녕하세요. 오늘 회의에서는 다음 달 출시 일정을 논의하겠습니다.", "ko-KR", &ko_wav).is_none() {
        eprintln!("한국어 TTS 음성 없음 - 건너뜀");
        return;
    }
    let ko_mp3 = dir.join("회의 녹음 한국어.mp3");
    convert(&ko_wav, &ko_mp3, &["-c:a", "libmp3lame", "-b:a", "96k"]);
    let (ko_text, ko_lang) = run_pipeline(&env.whisper, &env.tools, &ko_mp3, &dir, Language::Korean);
    eprintln!("[ko] {ko_text}");
    assert!(ko_text.contains("안녕하세요"), "한국어 인식 결과: {ko_text}");
    assert!(ko_text.contains("회의"), "한국어 인식 결과: {ko_text}");
    assert_eq!(ko_lang.as_deref(), Some("ko"));
    assert!(dir.join("회의 녹음 한국어.txt").is_file());

    // 영어: MP4 동영상 (영상 + AAC), 언어 자동 감지
    let en_wav = dir.join("english source.wav");
    if tts("Hello everyone. Today we will talk about the weather forecast for next week.", "en-US", &en_wav)
        .is_none()
    {
        eprintln!("영어 TTS 음성 없음 - 건너뜀");
        return;
    }
    let en_mp4 = dir.join("english lecture.mp4");
    let status = Command::new(ffmpeg_exe())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=blue:s=160x120:r=10", "-i"])
        .arg(&en_wav)
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&en_mp4)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    let (en_text, en_lang) = run_pipeline(&env.whisper, &env.tools, &en_mp4, &dir, Language::Auto);
    eprintln!("[en] {en_text}");
    let lower = en_text.to_lowercase();
    assert!(lower.contains("weather"), "영어 인식 결과: {en_text}");
    assert!(lower.contains("hello"), "영어 인식 결과: {en_text}");
    assert_eq!(en_lang.as_deref(), Some("en"), "자동 감지 언어");

    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn transcription_can_be_cancelled() {
    let Some(env) = env() else { return };
    let cancel = Arc::new(AtomicBool::new(true));
    let silence = vec![0.0f32; 16_000 * 5];
    let result = env.whisper.transcribe(&silence, Language::English, 2, cancel, None, |_| {});
    assert!(matches!(result, Err(super::SpeechError::Cancelled)));
}

#[test]
fn missing_model_gives_clear_error() {
    let err = LocalWhisper::load(Path::new(r"C:\없는 폴더\ggml-base.bin"), MODEL).err().expect("error");
    assert!(matches!(err, super::SpeechError::ModelMissing(_)));
    assert!(err.to_string().contains("모델 파일이 없습니다"));
}

#[test]
fn corrupted_model_fails_to_load() {
    let dir = temp_dir("corrupt_model");
    let path = dir.join("ggml-base.bin");
    std::fs::write(&path, vec![0u8; 4096]).expect("write");
    let result = LocalWhisper::load(&path, MODEL);
    assert!(matches!(result, Err(super::SpeechError::ModelLoad { .. })));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}
