//! 项目扫描器内核。
//!
//! 内核不持有进程状态：每次调用从磁盘读，写回磁盘。MCP 服务、命令行、将来的界面
//! 都只调用这里的函数。

pub mod anchor;
pub mod api;
pub mod error;
pub mod gates;
pub mod identity;
pub mod inventory;
pub mod log;
pub mod model;
pub mod pack;
pub mod record;
pub mod progress;
pub mod render;
pub mod sample;
pub mod status;
pub mod store;
pub mod templates;
pub mod text;

pub use error::{KError, KResult};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 一次性生成 RFC3339 时间戳（毫秒，UTC）。
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// sha256 十六进制。
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(data))
}

/// 规范 JSON（键按插入顺序，紧凑）的摘要，用于检查点幂等。
pub fn digest_json(v: &serde_json::Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    sha256_hex(s.as_bytes())
}
