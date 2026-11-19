//! 시스템 성능 정보 수집 (CPU, RAM, GPU, 디스크).
//!
//! 수집에 수십~수백 ms 가 걸릴 수 있으므로 GUI 스레드가 아닌 백그라운드에서 호출한다.

pub mod gpu;
pub mod hardware;
pub mod memory;

use std::path::Path;

pub use gpu::GpuInfo;

/// 엔진 추천에 쓰는 하드웨어 요약.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemProfile {
    pub cpu_brand: String,
    pub logical_cores: usize,
    pub physical_cores: Option<usize>,
    pub total_ram: u64,
    pub available_ram: u64,
    /// 소프트웨어 렌더러를 제외한 GPU 목록 (전용 VRAM 큰 순).
    pub gpus: Vec<GpuInfo>,
    /// 모델 저장 폴더가 있는 드라이브의 여유 공간.
    pub models_disk_free: Option<u64>,
}

/// 하드웨어 정보를 수집한다. 일부 항목을 읽지 못해도 나머지는 채운다.
pub fn collect(models_dir: &Path) -> SystemProfile {
    let started = std::time::Instant::now();
    let cpu = hardware::cpu_info();
    let mem = memory::memory_info();
    let gpus = gpu::list_gpus();
    let models_disk_free = hardware::disk_free_for(models_dir);
    tracing::info!(
        cpu = %cpu.brand,
        logical = cpu.logical_cores,
        physical = ?cpu.physical_cores,
        ram_total_mb = mem.total / (1024 * 1024),
        ram_available_mb = mem.available / (1024 * 1024),
        gpus = gpus.len(),
        elapsed_ms = started.elapsed().as_millis(),
        "시스템 정보 수집"
    );
    SystemProfile {
        cpu_brand: cpu.brand,
        logical_cores: cpu.logical_cores,
        physical_cores: cpu.physical_cores,
        total_ram: mem.total,
        available_ram: mem.available,
        gpus,
        models_disk_free,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_real_system_info() {
        let profile = collect(&std::env::temp_dir());
        assert!(profile.logical_cores >= 1);
        assert!(profile.total_ram > 512 * 1024 * 1024, "RAM {}", profile.total_ram);
        assert!(profile.available_ram <= profile.total_ram);
        assert!(profile.models_disk_free.is_some());
        eprintln!("{profile:#?}");
    }
}
