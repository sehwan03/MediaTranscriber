//! GPU 정보 (Windows DXGI).
//!
//! 현재 빌드는 CPU 추론만 사용한다. GPU 정보는 표시 및 추후 GPU 가속(cuda/vulkan) 확장 판단용이다.

#[derive(Debug, Clone, PartialEq)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: GpuVendor,
    /// 전용 VRAM (바이트). 내장 GPU 는 작거나 0 일 수 있다.
    pub dedicated_vram: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl GpuVendor {
    fn from_pci_id(id: u32) -> Self {
        match id {
            0x10DE => Self::Nvidia,
            0x1002 | 0x1022 => Self::Amd,
            0x8086 => Self::Intel,
            _ => Self::Other,
        }
    }
}

#[cfg(windows)]
pub fn list_gpus() -> Vec<GpuInfo> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};

    // SAFETY: COM 팩토리 생성 및 열거. 반환된 인터페이스는 drop 시 Release 된다.
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(f) => f,
        Err(err) => {
            tracing::warn!(error = %err, "DXGI 팩토리 생성 실패 (GPU 정보 없음)");
            return Vec::new();
        }
    };
    let mut gpus = Vec::new();
    for index in 0.. {
        // SAFETY: 범위를 벗어나면 DXGI_ERROR_NOT_FOUND 로 끝난다.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else { break };
        // SAFETY: 유효한 어댑터 인터페이스
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
            continue; // Microsoft Basic Render Driver 등
        }
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..len]).trim().to_owned();
        gpus.push(GpuInfo {
            name,
            vendor: GpuVendor::from_pci_id(desc.VendorId),
            dedicated_vram: desc.DedicatedVideoMemory as u64,
        });
    }
    gpus.sort_by_key(|g| std::cmp::Reverse(g.dedicated_vram));
    gpus.dedup_by(|a, b| a.name == b.name && a.dedicated_vram == b.dedicated_vram);
    gpus
}

#[cfg(not(windows))]
pub fn list_gpus() -> Vec<GpuInfo> {
    Vec::new()
}
