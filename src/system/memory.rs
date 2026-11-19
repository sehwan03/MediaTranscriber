use sysinfo::System;

pub struct MemoryInfo {
    pub total: u64,
    pub available: u64,
}

/// 전체/사용 가능 RAM (바이트). 읽지 못하면 0.
pub fn memory_info() -> MemoryInfo {
    match System::new() {
        Ok(mut sys) => {
            sys.refresh_memory();
            MemoryInfo { total: sys.total_memory(), available: sys.available_memory() }
        }
        Err(err) => {
            tracing::warn!(error = %err, "메모리 정보 읽기 실패");
            MemoryInfo { total: 0, available: 0 }
        }
    }
}

/// 현재 사용 가능한 RAM 만 빠르게 다시 읽는다 (추천 직전 갱신용).
pub fn available_ram() -> Option<u64> {
    System::new().ok().map(|mut sys| {
        sys.refresh_memory();
        sys.available_memory()
    })
}
