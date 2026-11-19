use std::path::Path;

use sysinfo::{CpuRefreshKind, Disks, System};

pub struct CpuInfo {
    pub brand: String,
    pub logical_cores: usize,
    pub physical_cores: Option<usize>,
}

pub fn cpu_info() -> CpuInfo {
    let logical_cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let brand = match System::new() {
        Ok(mut sys) => {
            sys.refresh_cpu_list(CpuRefreshKind::nothing());
            sys.cpus().first().map(|c| c.brand().trim().to_owned()).unwrap_or_default()
        }
        Err(err) => {
            tracing::warn!(error = %err, "CPU 정보 읽기 실패");
            String::new()
        }
    };
    CpuInfo {
        brand: if brand.is_empty() { "알 수 없는 CPU".to_owned() } else { brand },
        logical_cores,
        physical_cores: System::physical_core_count().ok(),
    }
}

/// `path` 가 속한 드라이브의 여유 공간. 경로가 아직 없어도 드라이브 기준으로 찾는다.
pub fn disk_free_for(path: &Path) -> Option<u64> {
    let disks = match Disks::new_with_refreshed_list() {
        Ok(d) => d,
        Err(err) => {
            tracing::warn!(error = %err, "디스크 정보 읽기 실패");
            return None;
        }
    };
    let target = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    disks
        .iter()
        .filter(|d| target.starts_with(d.mount_point()))
        // 가장 구체적인(긴) 마운트 지점을 선택
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}
