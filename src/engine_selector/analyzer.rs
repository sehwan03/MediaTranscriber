//! 로컬 처리 가능 여부와 예상 처리 시간 분석.
//!
//! - 속도는 "실시간 대비 배속(RTF)" = 음성 길이 / 처리 시간 으로 표현한다 (2.0 = 1분 음성을 30초에 처리).
//! - 벤치마크 측정값이 있으면 그것을 쓰고, 없으면 CPU 스레드 수 기반 추정치를 쓴다 (추정치임을 표시).
//! - PC 사양만으로 실제 속도를 확정하지 않는다.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::WhisperModel;
use crate::system::SystemProfile;

/// 앱·FFmpeg·디코딩 버퍼 등 모델 외 여유 메모리 (MiB).
const MEMORY_MARGIN_MB: u64 = 400;
/// 16 kHz mono f32 오디오 1초가 차지하는 메모리 (바이트).
const PCM_BYTES_PER_SEC: u64 = 16_000 * 4;
/// 모델 다운로드 시 필요한 여유 공간 (모델 크기 외).
const DISK_MARGIN: u64 = 256 * 1024 * 1024;

/// 저장되는 벤치마크 결과 (settings.json, 민감 정보 아님).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub model: WhisperModel,
    /// 실시간 대비 배속 (음성 초 / 처리 초)
    pub realtime_factor: f32,
    pub threads: u32,
    /// 실제 음성 파일 샘플로 측정했는지 (false = 합성 신호, 정확도 낮음)
    pub real_audio: bool,
    /// 측정 시각 (UNIX 초)
    pub measured_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpeedSource {
    /// 이 모델을 직접 측정
    Measured,
    /// 다른 모델 측정값에서 비율로 환산
    Derived,
    /// 측정 없이 CPU 정보로 추정
    Estimated,
}

impl SpeedSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Measured => "측정값",
            Self::Derived => "다른 모델 측정값 기반 추정",
            Self::Estimated => "추정치 (측정 전)",
        }
    }
}

/// 8 스레드 최신 데스크톱 CPU 기준 대략적 배속 (whisper.cpp CPU, greedy).
/// 모델 간 상대 비율 계산에도 사용한다.
fn baseline_rtf(model: WhisperModel) -> f32 {
    match model {
        WhisperModel::Tiny => 20.0,
        WhisperModel::Base => 8.0,
        WhisperModel::Small => 3.0,
        WhisperModel::Medium => 1.0,
    }
}

/// 모델의 예상 배속.
pub fn local_speed(
    model: WhisperModel,
    profile: &SystemProfile,
    benchmarks: &[BenchmarkResult],
) -> (f32, SpeedSource) {
    if let Some(b) = benchmarks.iter().find(|b| b.model == model) {
        return (b.realtime_factor, SpeedSource::Measured);
    }
    // 다른 모델 측정값이 있으면 모델 간 비율로 환산 (실제 기기 성능이 반영됨)
    if let Some(b) = benchmarks.iter().max_by_key(|b| b.measured_at) {
        let rtf = b.realtime_factor * baseline_rtf(model) / baseline_rtf(b.model);
        return (rtf, SpeedSource::Derived);
    }
    let threads = profile.logical_cores.clamp(1, 8) as f32;
    let mut rtf = baseline_rtf(model) * threads / 8.0;
    // 물리 코어가 매우 적은 저사양 CPU 는 하이퍼스레딩 효과가 작다
    if profile.physical_cores.is_some_and(|p| p <= 2) {
        rtf *= 0.6;
    }
    (rtf, SpeedSource::Estimated)
}

/// 모델 실행에 필요한 메모리 (바이트). 한 번에 메모리에 올리는 PCM 버퍼를 포함한다.
/// 긴 파일은 구간별로 처리하므로 버퍼는 구간 처리 기준 길이 이하로 제한된다.
pub fn required_memory(model: WhisperModel, longest_audio: Duration) -> u64 {
    let model_bytes = u64::from(model.approx_memory_mb()) * 1024 * 1024;
    let buffered = longest_audio.min(crate::task::LOCAL_CHUNK_THRESHOLD);
    model_bytes + MEMORY_MARGIN_MB * 1024 * 1024 + buffered.as_secs() * PCM_BYTES_PER_SEC
}

pub fn fits_in_memory(model: WhisperModel, profile: &SystemProfile, longest_audio: Duration) -> bool {
    profile.available_ram >= required_memory(model, longest_audio)
}

/// 미설치 모델을 내려받을 공간이 있는지. 디스크 정보를 모르면 true (다운로드 시 오류로 처리).
pub fn can_download(model_size: u64, profile: &SystemProfile) -> bool {
    profile.models_disk_free.is_none_or(|free| free >= model_size + DISK_MARGIN)
}

pub fn estimate_time(total_audio: Duration, rtf: f32) -> Duration {
    if rtf <= 0.0 {
        return Duration::MAX;
    }
    Duration::from_secs_f64(total_audio.as_secs_f64() / f64::from(rtf))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn profile(cores: usize, avail_gb: u64) -> SystemProfile {
        SystemProfile {
            cpu_brand: "Test CPU".to_owned(),
            logical_cores: cores,
            physical_cores: Some(cores / 2),
            total_ram: avail_gb * 2 * 1024 * 1024 * 1024,
            available_ram: avail_gb * 1024 * 1024 * 1024,
            gpus: Vec::new(),
            models_disk_free: Some(100 * 1024 * 1024 * 1024),
        }
    }

    fn bench(model: WhisperModel, rtf: f32) -> BenchmarkResult {
        BenchmarkResult { model, realtime_factor: rtf, threads: 8, real_audio: true, measured_at: 1 }
    }

    #[test]
    fn speed_prefers_measurement_then_derivation() {
        let p = profile(8, 8);
        let (rtf, src) = local_speed(WhisperModel::Base, &p, &[]);
        assert_eq!(src, SpeedSource::Estimated);
        assert!((rtf - 8.0).abs() < 1e-3);

        let measured = [bench(WhisperModel::Base, 12.0)];
        assert_eq!(local_speed(WhisperModel::Base, &p, &measured), (12.0, SpeedSource::Measured));
        let (small, src) = local_speed(WhisperModel::Small, &p, &measured);
        assert_eq!(src, SpeedSource::Derived);
        assert!((small - 4.5).abs() < 1e-3, "{small}");
    }

    #[test]
    fn low_core_cpu_is_slower() {
        let (fast, _) = local_speed(WhisperModel::Small, &profile(16, 8), &[]);
        let (slow, _) = local_speed(WhisperModel::Small, &profile(2, 8), &[]);
        assert!(slow < fast / 4.0, "{slow} vs {fast}");
    }

    #[test]
    fn memory_requirement_includes_audio_buffer() {
        let one_hour = Duration::from_secs(3600);
        let need = required_memory(WhisperModel::Base, one_hour);
        // 388 + 400 MB + 구간 처리 상한 20분 PCM(약 73 MB) — 1시간 파일도 전체를 올리지 않는다
        assert!(need > 850 * 1024 * 1024 && need < 900 * 1024 * 1024, "{need}");
        let short = required_memory(WhisperModel::Base, Duration::from_secs(60));
        assert!(short < need);
        assert!(fits_in_memory(WhisperModel::Medium, &profile(8, 4), one_hour));
        assert!(!fits_in_memory(WhisperModel::Medium, &profile(8, 2), one_hour));
    }

    #[test]
    fn time_estimate() {
        assert_eq!(estimate_time(Duration::from_secs(600), 2.0), Duration::from_secs(300));
        assert_eq!(estimate_time(Duration::from_secs(1), 0.0), Duration::MAX);
    }
}
