//! 임시 파일 가드: 값이 사라질 때(성공·실패·취소·패닉 모두) 파일을 지운다.
//!
//! 이름은 `접두어-프로세스ID-시각(ns)-순번.확장자` 로 만들어 동시 작업끼리 충돌하지 않는다.
//! 앱이 강제 종료되어 남은 파일은 다음 실행 때 [`cleanup_stale`] 로 지운다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct TempFile {
    path: PathBuf,
}

/// 겹치지 않는 이름: `접두어-프로세스ID-시각(ns)-순번`
fn unique_name(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{nanos}-{seq}", std::process::id())
}

impl TempFile {
    pub fn new_in(dir: &Path, prefix: &str, ext: &str) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self { path: dir.join(format!("{}.{ext}", unique_name(prefix))) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => tracing::debug!("임시 파일 삭제"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(file = %self.path.display(), error = %e, "임시 파일 삭제 실패"),
        }
    }
}

/// 임시 폴더 가드: 값이 사라질 때 폴더와 안의 파일을 모두 지운다.
/// (외부 도구가 파일 이름·확장자를 정하는 경우에 쓴다)
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new_in(dir: &Path, prefix: &str) -> std::io::Result<Self> {
        let path = dir.join(unique_name(prefix));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.path) {
            Ok(()) => tracing::debug!("임시 폴더 삭제"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(dir = %self.path.display(), error = %e, "임시 폴더 삭제 실패"),
        }
    }
}

/// 이전 실행에서 남은 임시 파일·폴더(`접두어-*`)를 지운다. 지운 개수를 돌려준다.
pub fn cleanup_stale(dir: &Path, prefix: &str) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let is_ours = name.to_str().is_some_and(|n| n.starts_with(&format!("{prefix}-")));
        // 링크는 따라가지 않는다 (file_type 은 링크 자체의 종류)
        let ok = match entry.file_type() {
            Ok(t) if is_ours && t.is_file() => std::fs::remove_file(entry.path()).is_ok(),
            Ok(t) if is_ours && t.is_dir() => std::fs::remove_dir_all(entry.path()).is_ok(),
            _ => false,
        };
        if ok {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_removes_file_and_names_are_unique() {
        let dir = std::env::temp_dir().join(format!("media_transcriber_tmpguard_{}", std::process::id()));
        let a = TempFile::new_in(&dir, "url", "wav").expect("a");
        let b = TempFile::new_in(&dir, "url", "wav").expect("b");
        assert_ne!(a.path(), b.path());
        std::fs::write(a.path(), b"x").expect("write");
        let p = a.path().to_path_buf();
        drop(a);
        assert!(!p.exists(), "가드가 사라지면 파일도 지워져야 함");
        drop(b); // 파일이 없어도 문제없음

        let t = TempDir::new_in(&dir, "url").expect("tempdir");
        std::fs::write(t.path().join("media.mp4"), b"x").expect("write");
        let tp = t.path().to_path_buf();
        drop(t);
        assert!(!tp.exists(), "임시 폴더는 안의 파일과 함께 지워져야 함");

        std::fs::write(dir.join("url-999-1-1.wav"), b"x").expect("write");
        std::fs::create_dir_all(dir.join("url-999-1-2")).expect("mkdir");
        std::fs::write(dir.join("url-999-1-2").join("media.webm"), b"x").expect("write");
        std::fs::write(dir.join("keep.txt"), b"x").expect("write");
        assert_eq!(cleanup_stale(&dir, "url"), 2);
        assert!(dir.join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
