//! Auto 모드: PC 성능 분석과 엔진 추천.

pub mod analyzer;
pub mod selector;

pub use analyzer::BenchmarkResult;
pub use selector::{Choice, Recommendation, SelectorInput, recommend};
