//! 작업 관리자 통합 테스트 (로컬 모의 HTTP 서버, 실제 외부 호출 없음).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::{JobEngine, TaskEvent, TaskManager, TranscribeJob};
use crate::config::secret::ApiKey;
use crate::config::settings::{CloudSettings, ProviderSettings};
use crate::config::{CloudProvider, Language, WhisperModel};

/// 요청마다 `delay` 만큼 붙잡고 있다가 응답하는 서버. (현재 동시 처리 수, 최대 동시 처리 수, 총 요청 수)
struct SlowServer {
    url: String,
    max_active: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
}

fn slow_server(delay: Duration) -> SlowServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/v1", listener.local_addr().expect("addr"));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let total = Arc::new(AtomicUsize::new(0));
    let (a, m, t) = (Arc::clone(&active), Arc::clone(&max_active), Arc::clone(&total));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let (a, m, t) = (Arc::clone(&a), Arc::clone(&m), Arc::clone(&t));
            std::thread::spawn(move || {
                let mut reader = BufReader::new(&stream);
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let l = line.trim_end();
                    if l.is_empty() {
                        break;
                    }
                    if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; len];
                let _ = reader.read_exact(&mut body);
                let now = a.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(now, Ordering::SeqCst);
                t.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(delay);
                a.fetch_sub(1, Ordering::SeqCst);
                let body = r#"{"text":"ok"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let mut s = stream;
                let _ = s.write_all(resp.as_bytes());
            });
        }
    });
    SlowServer { url, max_active, total }
}

fn ffmpeg_exe() -> PathBuf {
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join(r"Microsoft\WinGet\Links"));
    }
    dirs.into_iter().map(|d| d.join("ffmpeg.exe")).find(|p| p.is_file()).expect("ffmpeg.exe")
}

fn setup(name: &str) -> Option<(PathBuf, PathBuf)> {
    crate::media::FfmpegTools::locate(None).ok()?;
    let dir = std::env::temp_dir()
        .join(format!("media_transcriber_queue_{name}_{}", std::process::id()))
        .join("큐 테스트");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let input = dir.join("입력 음성.mp3");
    let ok = std::process::Command::new(ffmpeg_exe())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=duration=3", "-c:a", "libmp3lame"])
        .arg(&input)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some((dir, input))
}

fn cloud_job(id: u64, input: &Path, out: &Path, url: &str, concurrency: u32) -> (TranscribeJob, Arc<AtomicBool>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let settings = CloudSettings {
        provider: CloudProvider::OpenAi,
        openai: ProviderSettings { base_url: url.to_owned(), model: "gpt-transcribe".to_owned() },
        max_concurrent_requests: concurrency,
        ..CloudSettings::default()
    };
    let job = TranscribeJob {
        id,
        input: input.to_path_buf(),
        url: None,
        duration: None,
        output_dir: out.to_path_buf(),
        engine: JobEngine::Cloud { settings, api_key: ApiKey::new("fake-queue-test-key-000").expect("key") },
        language: Language::Auto,
        ffmpeg_dir: None,
        cancel: Arc::clone(&cancel),
    };
    (job, cancel)
}

/// 완료/실패/취소 이벤트를 `count` 개 모을 때까지 기다린다.
fn collect_finished(manager: &TaskManager, count: usize, timeout: Duration) -> Vec<TaskEvent> {
    let deadline = Instant::now() + timeout;
    let mut done = Vec::new();
    while done.len() < count && Instant::now() < deadline {
        match manager.try_recv() {
            Some(e @ (TaskEvent::Completed { .. } | TaskEvent::Failed { .. } | TaskEvent::Cancelled { .. })) => done.push(e),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    done
}

#[test]
fn cloud_jobs_respect_concurrency_limit() {
    let Some((dir, input)) = setup("limit") else { return };
    let server = slow_server(Duration::from_millis(400));
    let manager = TaskManager::spawn(|| {}).expect("spawn");
    for id in 1..=5 {
        let (job, _) = cloud_job(id, &input, &dir, &server.url, 2);
        manager.submit(job).expect("submit");
    }
    let done = collect_finished(&manager, 5, Duration::from_secs(30));
    assert_eq!(done.len(), 5);
    assert!(done.iter().all(|e| matches!(e, TaskEvent::Completed { .. })));
    assert_eq!(server.max_active.load(Ordering::SeqCst), 2, "동시 요청은 설정값(2)을 넘지 않고, 실제로 병렬 처리되어야 함");
    // 같은 이름 입력 5개 → 덮어쓰지 않고 5개 파일
    let txts = std::fs::read_dir(&dir).expect("dir").filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == "txt")).count();
    assert_eq!(txts, 5);
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn queued_cloud_job_cancelled_before_start_is_not_sent() {
    let Some((dir, input)) = setup("cancel") else { return };
    let server = slow_server(Duration::from_millis(300));
    let manager = TaskManager::spawn(|| {}).expect("spawn");
    let (job1, _) = cloud_job(1, &input, &dir, &server.url, 1);
    let (job2, cancel2) = cloud_job(2, &input, &dir, &server.url, 1);
    let (job3, _) = cloud_job(3, &input, &dir, &server.url, 1);
    manager.submit(job1).expect("submit");
    manager.submit(job2).expect("submit");
    manager.submit(job3).expect("submit");
    // 2번은 대기 중에 취소
    cancel2.store(true, Ordering::SeqCst);
    let done = collect_finished(&manager, 3, Duration::from_secs(30));
    assert_eq!(done.len(), 3);
    assert!(done.iter().any(|e| matches!(e, TaskEvent::Cancelled { id: 2 })));
    assert_eq!(done.iter().filter(|e| matches!(e, TaskEvent::Completed { .. })).count(), 2);
    assert_eq!(server.total.load(Ordering::SeqCst), 2, "취소된 작업은 전송하지 않음");
    assert_eq!(server.max_active.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

#[test]
fn failed_local_job_does_not_stop_next_jobs() {
    let Some((dir, input)) = setup("isolation") else { return };
    let manager = TaskManager::spawn(|| {}).expect("spawn");
    for id in 1..=2 {
        manager
            .submit(TranscribeJob {
                id,
                input: if id == 1 { dir.join("없는 파일.mp3") } else { input.clone() },
                url: None,
                duration: None,
                output_dir: dir.clone(),
                // 2번도 없는 모델 경로 → 실패하지만, 1번 실패 후에도 처리되어야 한다
                engine: JobEngine::Local { model: WhisperModel::Tiny, model_path: dir.join("no-model.bin") },
                language: Language::Auto,
                ffmpeg_dir: None,
                cancel: Arc::new(AtomicBool::new(false)),
            })
            .expect("submit");
    }
    let done = collect_finished(&manager, 2, Duration::from_secs(20));
    assert_eq!(done.len(), 2, "첫 작업 실패 후에도 다음 작업이 처리되어야 함");
    for e in &done {
        match e {
            TaskEvent::Failed { error, .. } => assert!(error.contains("모델") || error.contains("파일"), "{error}"),
            _ => panic!("실패 이벤트여야 함"),
        }
    }
    let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
}

/// 어떤 요청이든 {"text": text} 로 답하는 모의 OpenAI 서버. 반환: base_url
pub(crate) fn text_server(text: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/v1", listener.local_addr().expect("addr"));
    let body = serde_json::json!({ "text": text }).to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let body = body.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(&stream);
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let l = line.trim_end();
                    if l.is_empty() {
                        break;
                    }
                    if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut buf = vec![0u8; len];
                let _ = reader.read_exact(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let mut s = stream;
                let _ = s.write_all(resp.as_bytes());
            });
        }
    });
    url
}
