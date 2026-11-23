#![forbid(unsafe_code)]
#[cfg(feature = "desktop")]
pub mod app;
pub mod content;
pub mod engine;
pub mod export;
pub mod model;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
