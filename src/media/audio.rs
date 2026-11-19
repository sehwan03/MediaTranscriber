use std::io::Read;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::ffmpeg::{check_input_file, stderr_summary};
use super::{FfmpegTools, MediaError, WHISPER_SAMPLE_RATE};

/// stdout 읽기 버퍼 크기 (16 kHz f32 기준 약 1초 분량).
const READ_CHUNK_BYTES: usize = 64 * 1024;
/// stderr 는 오류 메시지 표시용으로 끝부분만 보관한다.
const STDERR_KEEP_BYTES: usize = 8 * 1024;
/// 길이 힌트로 미리 확보할 최대 샘플 수 (약 4시간). 비정상 메타데이터로 과다 할당 방지.
const MAX_PREALLOC_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 60 * 60 * 4;

/// 디코딩된 Whisper 입력용 오디오 (16 kHz, mono, f32 [-1, 1]).
#[derive(Debug)]
pub struct PcmAudio {
    pub samples: Vec<f32>,
}

/// 입력 파일의 첫 번째 오디오 트랙을 16 kHz mono f32 로 디코딩해 메모리로 받는다.
///
/// - 임시 파일 없이 ffmpeg stdout 파이프로 받는다.
/// - `cancel` 이 설정되면 ffmpeg 프로세스를 종료하고 `Cancelled` 를 반환한다.
/// - `on_progress` 는 0.0~1.0 (길이를 알 때만 의미 있음).
pub fn decode_to_whisper_pcm(
    tools: &FfmpegTools,
    path: &Path,
    duration_hint: Option<Duration>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<PcmAudio, MediaError> {
    let expected = duration_hint
        .map(|d| (d.as_secs_f64() * f64::from(WHISPER_SAMPLE_RATE)) as usize)
        .unwrap_or(0);
    let mut samples = Vec::with_capacity(expected.min(MAX_PREALLOC_SAMPLES) + READ_CHUNK_BYTES / 4);

    decode_stream(tools, path, Duration::ZERO, None, cancel, |chunk| {
        samples.extend_from_slice(chunk);
        if expected > 0 {
            on_progress((samples.len() as f32 / expected as f32).min(1.0));
        }
    })?;

    if samples.is_empty() {
        return Err(MediaError::EmptyAudio);
    }
    on_progress(1.0);
    Ok(PcmAudio { samples })
}

/// [start, start+limit) 구간만 16 kHz mono f32 로 디코딩한다 (긴 파일 구간 처리, 성능 측정 샘플).
pub fn decode_range_pcm(
    tools: &FfmpegTools,
    path: &Path,
    start: Duration,
    limit: Duration,
    cancel: &AtomicBool,
) -> Result<Vec<f32>, MediaError> {
    let mut samples = Vec::with_capacity((limit.as_secs_f64() * f64::from(WHISPER_SAMPLE_RATE)) as usize);
    decode_stream(tools, path, start, Some(limit), cancel, |chunk| samples.extend_from_slice(chunk))?;
    if samples.is_empty() {
        return Err(MediaError::EmptyAudio);
    }
    Ok(samples)
}

/// 디코딩된 샘플을 청크 단위로 콜백에 전달한다.
/// 이후 긴 파일을 구간별로 처리할 때 전체를 메모리에 올리지 않고 쓸 수 있다.
pub fn decode_stream(
    tools: &FfmpegTools,
    path: &Path,
    start: Duration,
    limit: Option<Duration>,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(&[f32]),
) -> Result<(), MediaError> {
    let path = check_input_file(path)?;
    let sample_rate = WHISPER_SAMPLE_RATE.to_string();
    let mut cmd = tools.ffmpeg_command();
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    if !start.is_zero() {
        cmd.args(["-ss", &format!("{:.3}", start.as_secs_f64())]);
    }
    cmd.arg("-i").arg(&path);
    if let Some(limit) = limit {
        cmd.args(["-t", &format!("{:.3}", limit.as_secs_f64())]);
    }
    cmd.args([
            "-map", "0:a:0", // 첫 번째 오디오 트랙
            "-vn", "-sn", "-dn",
            "-ac", "1",
            "-ar", &sample_rate,
            "-f", "f32le",
            "pipe:1",
        ]);

    let mut floats: Vec<f32> = Vec::with_capacity(READ_CHUNK_BYTES / 4);
    // 4 바이트 경계에 걸친 나머지 바이트
    let mut carry = [0u8; 4];
    let mut carry_len = 0usize;
    run_ffmpeg(cmd, cancel, &mut |bytes: &[u8]| {
        floats.clear();
        let mut data = bytes;
        if carry_len > 0 {
            let need = 4 - carry_len;
            if data.len() < need {
                carry[carry_len..carry_len + data.len()].copy_from_slice(data);
                carry_len += data.len();
                return;
            }
            carry[carry_len..].copy_from_slice(&data[..need]);
            floats.push(f32::from_le_bytes(carry));
            data = &data[need..];
        }
        let (chunks, rest) = data.as_chunks::<4>();
        floats.extend(chunks.iter().map(|b| f32::from_le_bytes(*b)));
        carry[..rest.len()].copy_from_slice(rest);
        carry_len = rest.len();
        if !floats.is_empty() {
            on_chunk(&floats);
        }
    })
}

/// 입력 파일의 [start, start+length) 구간 오디오를 Cloud 업로드용 MP3 로 인코딩해 메모리로 받는다.
/// (16 kHz mono, 48 kbps → 10분 약 3.6 MB). 임시 파일을 만들지 않는다.
pub fn encode_segment_mp3(
    tools: &FfmpegTools,
    path: &Path,
    start: Duration,
    length: Option<Duration>,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, MediaError> {
    let path = check_input_file(path)?;
    let mut cmd = tools.ffmpeg_command();
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    if !start.is_zero() {
        cmd.args(["-ss", &format!("{:.3}", start.as_secs_f64())]);
    }
    cmd.arg("-i").arg(&path);
    if let Some(len) = length {
        cmd.args(["-t", &format!("{:.3}", len.as_secs_f64())]);
    }
    cmd.args([
        "-map", "0:a:0",
        "-vn", "-sn", "-dn",
        "-ac", "1",
        "-ar", "16000",
        "-c:a", "libmp3lame",
        "-b:a", "48k",
        "-f", "mp3",
        "pipe:1",
    ]);
    let mut out = Vec::new();
    run_ffmpeg(cmd, cancel, &mut |bytes: &[u8]| out.extend_from_slice(bytes))?;
    if out.is_empty() {
        return Err(MediaError::EmptyAudio);
    }
    Ok(out)
}

/// ffmpeg 를 실행하고 stdout 을 콜백으로 흘려보낸다. 취소 시 프로세스를 종료한다.
fn run_ffmpeg(
    mut cmd: std::process::Command,
    cancel: &AtomicBool,
    on_bytes: &mut dyn FnMut(&[u8]),
) -> Result<(), MediaError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(MediaError::Cancelled);
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| MediaError::Spawn { tool: "ffmpeg", source })?;

    // stderr 를 별도 스레드에서 비워야 파이프가 가득 차 ffmpeg 가 멈추지 않는다.
    let stderr_thread = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                kept.extend_from_slice(&buf[..n]);
                if kept.len() > STDERR_KEEP_BYTES * 2 {
                    kept.drain(..kept.len() - STDERR_KEEP_BYTES);
                }
            }
            kept
        })
    });

    let read_result = read_stdout(&mut child, cancel, on_bytes);
    if read_result.is_err() {
        kill_quietly(&mut child);
    }
    let status = child.wait();
    let stderr = stderr_thread.and_then(|t| t.join().ok()).unwrap_or_default();

    read_result?;
    let status = status?;
    if !status.success() {
        return Err(MediaError::DecodeFailed(stderr_summary(&stderr)));
    }
    Ok(())
}

fn read_stdout(
    child: &mut Child,
    cancel: &AtomicBool,
    on_bytes: &mut dyn FnMut(&[u8]),
) -> Result<(), MediaError> {
    let Some(stdout) = child.stdout.as_mut() else {
        return Err(MediaError::DecodeFailed("ffmpeg 출력 파이프를 열 수 없습니다".to_owned()));
    };
    let mut buf = vec![0u8; READ_CHUNK_BYTES];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        match stdout.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => on_bytes(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(MediaError::Io(e)),
        }
    }
}

fn kill_quietly(child: &mut Child) {
    if let Err(err) = child.kill() {
        // 이미 종료된 경우 등. 이후 wait 에서 정리된다.
        tracing::debug!(error = %err, "ffmpeg 프로세스 종료 요청 실패");
    }
}
