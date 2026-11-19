//! 음성 인식 엔진.
//!
//! - `local`: whisper.cpp (whisper-rs) 로컬 인식
//! - `cloud`: OpenAI API (STEP 4)
//! - `model`: 로컬 모델 파일 관리

pub mod cloud;
pub mod engine;
pub mod local;
pub mod model;
pub mod model_worker;

pub use engine::{SpeechError, Transcript};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cloud_tests;
