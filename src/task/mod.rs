//! 변환 작업 처리. GUI 스레드와 분리된 작업 스레드에서 실행된다.
//!
//! - `manager`: GUI 가 쓰는 창구 (Local / Cloud 로 분배, 이벤트 수집)
//! - `worker` : Local 작업 스레드 (동시 1개, 모델 재사용, 긴 파일 구간 처리)
//! - `queue`  : Cloud 대기열 (설정된 동시 요청 수만큼 병렬)
//! - `job`    : 작업·이벤트 타입

mod input;
mod job;
mod manager;
mod queue;
mod worker;

pub use job::{BenchmarkJob, JobEngine, Stage, TaskEvent, TranscribeJob, UrlJob};
pub use manager::TaskManager;
pub use worker::LOCAL_CHUNK_THRESHOLD;

#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod url_tests;
