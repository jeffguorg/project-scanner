//! 结构性错误：不写入，返回给 agent 要改。码以 `E_` 开头。

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

impl KError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into(), data: Value::Null }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("E_INVALID_INPUT", message)
    }
}

impl std::fmt::Display for KError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for KError {}

pub type KResult<T> = Result<T, KError>;
