//! 화면 구성 요소. 각 패널은 `AppState` 를 받아 그리기만 하며,
//! 무거운 작업(FFmpeg, 추론, HTTP)은 절대 여기서 수행하지 않는다.

pub mod file_panel;
pub mod result_panel;
pub mod settings_panel;
pub mod task_panel;
