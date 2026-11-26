//! 调用日志：`log.jsonl`，只增不改。每次工具调用一条，非工具事件也记。

use crate::model::Gap;
use crate::store::ScanStore;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub seq: u64,
    pub at: String,
    /// call | event
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// ok | warnings | rejected | error
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<Gap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    /// takeover | nudge | seal | gate | reopen
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// mcp | cli | tui | plugin | kernel
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub detail: Value,
}

pub fn read_all(store: &ScanStore) -> Vec<LogEntry> {
    let Ok(text) = fs::read_to_string(store.log_path()) else {
        return vec![];
    };
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

pub fn append(store: &ScanStore, mut entry: LogEntry) -> Result<()> {
    let last_seq = fs::read_to_string(store.log_path())
        .ok()
        .and_then(|t| t.lines().rev().find(|l| !l.trim().is_empty()).and_then(|l| serde_json::from_str::<LogEntry>(l).ok()))
        .map(|e| e.seq)
        .unwrap_or(0);
    entry.seq = last_seq + 1;
    let mut f = OpenOptions::new().create(true).append(true).open(store.log_path())?;
    writeln!(f, "{}", serde_json::to_string(&entry)?)?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LogFilter {
    pub tool: Option<String>,
    pub last: Option<usize>,
    pub since: Option<String>,
}

impl Default for LogFilter {
    fn default() -> Self {
        Self { tool: None, last: None, since: None }
    }
}

pub fn filter(entries: Vec<LogEntry>, f: &LogFilter) -> Vec<LogEntry> {
    let mut out = entries;
    if let Some(t) = &f.tool {
        out.retain(|e| e.tool.as_deref() == Some(t.as_str()));
    }
    if let Some(s) = &f.since {
        out.retain(|e| e.at.as_str() >= s.as_str());
    }
    if let Some(n) = f.last {
        let len = out.len();
        out.drain(..len.saturating_sub(n));
    }
    out
}
