//! Auto 모드 엔진 추천.
//!
//! 판단 순서
//! 1. 메모리: 선택 모델이 현재 사용 가능 RAM 에 들어가는지 (안 되면 들어가는 더 작은 모델 제안)
//! 2. 설치/디스크: 모델이 없으면 다운로드 가능한 공간이 있는지
//! 3. 속도: 측정값(없으면 추정치) 기준 실시간보다 느리면 Cloud 를 추천
//! 4. Cloud 는 API Key 가 있을 때만 추천한다. 없으면 Local 로 진행하되 이유를 보여준다.
//!
//! 추천은 시작 시점에 한 번 결정하며, 처리 도중 네트워크 상태에 따라 몰래 바꾸지 않는다.

use std::time::Duration;

use super::analyzer::{self, BenchmarkResult, SpeedSource};
use crate::config::WhisperModel;
use crate::system::SystemProfile;

/// 이 배속보다 느리면 "로컬이 너무 느림"으로 본다 (1.0 = 실시간).
const MIN_LOCAL_RTF: f32 = 1.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Local { model: WhisperModel },
    Cloud,
    /// 로컬도 Cloud 도 사용할 수 없음
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalEstimate {
    pub model: WhisperModel,
    pub rtf: f32,
    pub source: SpeedSource,
    pub total_time: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recommendation {
    pub choice: Choice,
    /// 사용자에게 보여줄 판단 근거
    pub reasons: Vec<String>,
    pub local: Option<LocalEstimate>,
    /// Local 을 고른 경우 모델 다운로드가 먼저 필요한지
    pub needs_download: bool,
}

pub struct SelectorInput<'a> {
    pub profile: &'a SystemProfile,
    pub benchmarks: &'a [BenchmarkResult],
    pub preferred_model: WhisperModel,
    pub is_installed: &'a dyn Fn(WhisperModel) -> bool,
    pub model_size: &'a dyn Fn(WhisperModel) -> u64,
    /// 변환할 파일들의 총 길이 / 가장 긴 파일 길이 (없으면 0)
    pub total_audio: Duration,
    pub longest_audio: Duration,
    pub cloud_ready: bool,
    pub cloud_name: &'a str,
}

pub fn recommend(input: &SelectorInput<'_>) -> Recommendation {
    let mut reasons = Vec::new();
    let profile = input.profile;

    // 1. 메모리: 선호 모델부터 더 작은 모델 순으로 들어가는 것을 찾는다.
    let candidates: Vec<WhisperModel> =
        WhisperModel::ALL.iter().rev().copied().filter(|m| *m <= input.preferred_model).collect();
    let fitting = candidates.iter().copied().find(|m| analyzer::fits_in_memory(*m, profile, input.longest_audio));

    let local_model = match fitting {
        Some(m) if m == input.preferred_model => {
            reasons.push(format!(
                "사용 가능 RAM {:.1} GB 로 {} 모델(약 {} MB) 실행 가능",
                gb(profile.available_ram),
                m.label(),
                m.approx_memory_mb()
            ));
            Some(m)
        }
        Some(m) => {
            reasons.push(format!(
                "사용 가능 RAM {:.1} GB 가 부족해 {} 대신 {} 모델을 사용",
                gb(profile.available_ram),
                input.preferred_model.label(),
                m.label()
            ));
            Some(m)
        }
        None => {
            reasons.push(format!(
                "사용 가능 RAM {:.1} GB 로는 로컬 모델을 실행하기 어려움",
                gb(profile.available_ram)
            ));
            None
        }
    };

    // 2. 설치/디스크
    let (local_model, needs_download) = match local_model {
        Some(m) if (input.is_installed)(m) => (Some(m), false),
        Some(m) if analyzer::can_download((input.model_size)(m), profile) => {
            reasons.push(format!("{} 모델이 설치되어 있지 않아 먼저 다운로드가 필요", m.label()));
            (Some(m), true)
        }
        Some(m) => {
            reasons.push(format!("{} 모델을 내려받을 디스크 공간이 부족", m.label()));
            (None, false)
        }
        None => (None, false),
    };

    // 3. 속도
    let local = local_model.map(|model| {
        let (rtf, source) = analyzer::local_speed(model, profile, input.benchmarks);
        let total_time = (!input.total_audio.is_zero()).then(|| analyzer::estimate_time(input.total_audio, rtf));
        LocalEstimate { model, rtf, source, total_time }
    });
    let too_slow = local.as_ref().is_some_and(|l| l.rtf < MIN_LOCAL_RTF);
    if let Some(l) = &local {
        reasons.push(format!("로컬 처리 속도 약 {:.1}배속 ({})", l.rtf, l.source.label()));
        if too_slow {
            reasons.push("실시간보다 느려 긴 파일은 오래 걸릴 수 있음".to_owned());
        }
    }

    // 4. 결정
    let choice = match (&local, too_slow, input.cloud_ready) {
        (Some(l), false, _) => Choice::Local { model: l.model },
        (_, _, true) => {
            reasons.push(format!("{} Cloud 사용 권장 (전송 전 동의 필요)", input.cloud_name));
            Choice::Cloud
        }
        (Some(l), true, false) => {
            reasons.push(format!("Cloud 를 쓰려면 {} API Key 가 필요 — 로컬로 진행", input.cloud_name));
            Choice::Local { model: l.model }
        }
        (None, _, false) => {
            reasons.push(format!("로컬 실행 불가, Cloud 는 {} API Key 가 필요", input.cloud_name));
            Choice::Unavailable
        }
    };
    let needs_download = needs_download && matches!(choice, Choice::Local { .. });
    Recommendation { choice, reasons, local, needs_download }
}

fn gb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_selector::analyzer::tests::profile;

    fn run(
        p: &SystemProfile,
        benches: &[BenchmarkResult],
        preferred: WhisperModel,
        installed: &dyn Fn(WhisperModel) -> bool,
        cloud_ready: bool,
    ) -> Recommendation {
        recommend(&SelectorInput {
            profile: p,
            benchmarks: benches,
            preferred_model: preferred,
            is_installed: installed,
            model_size: &|_| 500 * 1024 * 1024,
            total_audio: Duration::from_secs(1800),
            longest_audio: Duration::from_secs(1800),
            cloud_ready,
            cloud_name: "OpenAI",
        })
    }

    #[test]
    fn high_end_pc_uses_local() {
        let r = run(&profile(16, 16), &[], WhisperModel::Small, &|_| true, true);
        assert_eq!(r.choice, Choice::Local { model: WhisperModel::Small });
        let est = r.local.expect("estimate");
        assert_eq!(est.source, SpeedSource::Estimated);
        // 30분 / 3배속 = 10분
        assert_eq!(est.total_time, Some(Duration::from_secs(600)));
    }

    #[test]
    fn low_memory_falls_back_to_smaller_model() {
        // 1.2 GB 사용 가능 → Medium(2.1GB) 불가, Small(0.85GB)+여유 불가, Base 가능
        let mut p = profile(8, 1);
        p.available_ram = 1200 * 1024 * 1024;
        let r = run(&p, &[], WhisperModel::Medium, &|_| true, false);
        assert_eq!(r.choice, Choice::Local { model: WhisperModel::Base });
        assert!(r.reasons.iter().any(|s| s.contains("대신")));
    }

    #[test]
    fn slow_pc_prefers_cloud_when_key_available() {
        let p = profile(2, 4);
        let slow = [BenchmarkResult {
            model: WhisperModel::Small,
            realtime_factor: 0.4,
            threads: 2,
            real_audio: true,
            measured_at: 1,
        }];
        let r = run(&p, &slow, WhisperModel::Small, &|_| true, true);
        assert_eq!(r.choice, Choice::Cloud);
        // 키가 없으면 로컬로 진행하되 이유를 남긴다 (무단 Cloud 전송 없음)
        let r = run(&p, &slow, WhisperModel::Small, &|_| true, false);
        assert_eq!(r.choice, Choice::Local { model: WhisperModel::Small });
        assert!(r.reasons.iter().any(|s| s.contains("API Key")));
    }

    #[test]
    fn missing_model_requires_download_or_disk() {
        let p = profile(16, 16);
        let r = run(&p, &[], WhisperModel::Base, &|_| false, false);
        assert!(r.needs_download);
        let mut full = profile(16, 16);
        full.models_disk_free = Some(10 * 1024 * 1024);
        let r = run(&full, &[], WhisperModel::Base, &|_| false, false);
        assert_eq!(r.choice, Choice::Unavailable);
        let r = run(&full, &[], WhisperModel::Base, &|_| false, true);
        assert_eq!(r.choice, Choice::Cloud);
    }
}
