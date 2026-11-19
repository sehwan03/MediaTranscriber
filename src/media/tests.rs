//! 실제 FFmpeg 를 사용하는 통합 테스트.
//! FFmpeg 를 찾지 못하면 테스트를 건너뛴다 (메시지 출력).
//! 테스트용 미디어는 ffmpeg lavfi 로 생성하므로 외부 파일이 필요 없다.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::audio::decode_to_whisper_pcm;
use super::{FfmpegTools, MediaError, WHISPER_SAMPLE_RATE};

const SECONDS: f64 = 3.0;

struct Fixture {
    dir: PathBuf,
    tools: FfmpegTools,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(name: &str) -> Option<Fixture> {
    let tools = match FfmpegTools::locate(None) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("FFmpeg 없음 - 테스트 건너뜀: {e}");
            return None;
        }
    };
    // 한글 + 공백이 포함된 경로에서 동작하는지 함께 검증한다.
    let dir = std::env::temp_dir()
        .join(format!("media_transcriber_media_{name}_{}", std::process::id()))
        .join("테스트 폴더");
    std::fs::create_dir_all(&dir).expect("mkdir");
    Some(Fixture { dir, tools })
}

/// lavfi 로 테스트 미디어를 생성한다 (테스트 전용 ffmpeg 호출).
fn generate(out: &Path, args: &[&str]) {
    let ffmpeg = find_ffmpeg_exe();
    let status = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .expect("ffmpeg 실행");
    assert!(status.success(), "테스트 미디어 생성 실패: {}", out.display());
}

fn find_ffmpeg_exe() -> PathBuf {
    // FfmpegTools 는 경로를 노출하지 않으므로 같은 규칙으로 PATH/winget 링크에서 찾는다.
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join(r"Microsoft\WinGet\Links"));
    }
    dirs.into_iter()
        .map(|d| d.join("ffmpeg.exe"))
        .find(|p| p.is_file())
        .expect("ffmpeg.exe")
}

const SINE: &[&str] = &["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100:duration=3"];

fn assert_decoded(tools: &FfmpegTools, path: &Path, expect_video: bool) {
    let info = tools.probe(path).expect("probe");
    assert_eq!(info.has_video, expect_video);
    let dur = info.duration.expect("duration").as_secs_f64();
    assert!((dur - SECONDS).abs() < 0.2, "probe 길이 {dur}");

    let cancel = AtomicBool::new(false);
    let mut last_progress = 0.0;
    let pcm = decode_to_whisper_pcm(tools, path, info.duration, &cancel, |p| last_progress = p)
        .expect("decode");

    let expected = SECONDS * f64::from(WHISPER_SAMPLE_RATE);
    let got = pcm.samples.len() as f64;
    // 코덱 패딩(MP3/AAC 앞뒤 무음) 허용 오차 5%
    assert!((got - expected).abs() / expected < 0.05, "샘플 수 {got} (기대 {expected})");
    assert!(pcm.samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0001));
    // 사인파이므로 무음이 아니어야 한다.
    let peak = pcm.samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "peak {peak}");
    assert!((last_progress - 1.0).abs() < f32::EPSILON);
}

#[test]
fn mp3_is_decoded_to_16k_mono_f32() {
    let Some(fx) = fixture("mp3") else { return };
    let path = fx.dir.join("한글 음성 파일.mp3");
    generate(&path, &[SINE, &["-c:a", "libmp3lame", "-b:a", "64k"]].concat());
    assert_decoded(&fx.tools, &path, false);
}

#[test]
fn mp4_video_audio_is_extracted() {
    let Some(fx) = fixture("mp4") else { return };
    let path = fx.dir.join("강의 영상 1.mp4");
    generate(
        &path,
        &[
            &["-f", "lavfi", "-i", "testsrc=duration=3:size=160x120:rate=10"][..],
            SINE,
            &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"],
        ]
        .concat(),
    );
    assert_decoded(&fx.tools, &path, true);
}

#[test]
fn wav_and_mkv_are_supported() {
    let Some(fx) = fixture("wav_mkv") else { return };
    let wav = fx.dir.join("녹음.wav");
    generate(&wav, SINE);
    assert_decoded(&fx.tools, &wav, false);

    let mkv = fx.dir.join("회의 녹화.mkv");
    generate(
        &mkv,
        &[
            &["-f", "lavfi", "-i", "testsrc=duration=3:size=160x120:rate=10"][..],
            SINE,
            &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "libopus", "-shortest"],
        ]
        .concat(),
    );
    assert_decoded(&fx.tools, &mkv, true);
}

#[test]
fn video_without_audio_is_rejected() {
    let Some(fx) = fixture("noaudio") else { return };
    let path = fx.dir.join("무음 영상.mp4");
    generate(
        &path,
        &["-f", "lavfi", "-i", "testsrc=duration=1:size=160x120:rate=10", "-c:v", "libx264", "-pix_fmt", "yuv420p"],
    );
    assert!(matches!(fx.tools.probe(&path), Err(MediaError::NoAudioStream)));
}

#[test]
fn empty_corrupt_missing_files_are_rejected() {
    let Some(fx) = fixture("bad") else { return };

    let empty = fx.dir.join("빈 파일.mp3");
    std::fs::write(&empty, b"").expect("write");
    assert!(matches!(fx.tools.probe(&empty), Err(MediaError::EmptyFile)));

    // 확장자는 mp3 지만 실제로는 텍스트
    let fake = fx.dir.join("가짜 음성.mp3");
    std::fs::write(&fake, "이것은 오디오가 아닙니다".repeat(100)).expect("write");
    assert!(matches!(fx.tools.probe(&fake), Err(MediaError::InvalidMedia(_))));

    // 앞부분만 남은 손상 파일
    let full = fx.dir.join("원본.mp3");
    generate(&full, &[SINE, &["-c:a", "libmp3lame"]].concat());
    let bytes = std::fs::read(&full).expect("read");
    let truncated = fx.dir.join("손상된 파일.mp4");
    std::fs::write(&truncated, &bytes[..64]).expect("write");
    assert!(fx.tools.probe(&truncated).is_err());

    let missing = fx.dir.join("없는 파일.wav");
    assert!(matches!(fx.tools.probe(&missing), Err(MediaError::FileNotFound(_))));
}

#[test]
fn decode_honors_cancellation() {
    let Some(fx) = fixture("cancel") else { return };
    let path = fx.dir.join("취소 테스트.wav");
    generate(&path, &["-f", "lavfi", "-i", "sine=frequency=440:duration=60"]);

    // 시작 전 취소
    let cancel = AtomicBool::new(true);
    let result = decode_to_whisper_pcm(&fx.tools, &path, None, &cancel, |_| {});
    assert!(matches!(result, Err(MediaError::Cancelled)));

    // 디코딩 도중 취소: 첫 진행률 보고 시점에 취소 플래그를 세운다.
    let cancel = AtomicBool::new(false);
    let result = decode_to_whisper_pcm(&fx.tools, &path, Some(Duration::from_secs(60)), &cancel, |_| {
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    assert!(matches!(result, Err(MediaError::Cancelled)));
}
