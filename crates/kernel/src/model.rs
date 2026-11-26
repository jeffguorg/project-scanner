//! 数据模型：scan.json、state.json、progress.json 的形状。JSON 一律 camelCase。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;

pub const SCHEMA: u32 = 1;

// ─── 阶段 ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Preflight,
    Planning,
    Discovery,
    Validation,
    AnalyzeFullPath,
    Finalized,
}

impl Phase {
    pub const ALL: [Phase; 6] = [
        Phase::Preflight,
        Phase::Planning,
        Phase::Discovery,
        Phase::Validation,
        Phase::AnalyzeFullPath,
        Phase::Finalized,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Preflight => "preflight",
            Phase::Planning => "planning",
            Phase::Discovery => "discovery",
            Phase::Validation => "validation",
            Phase::AnalyzeFullPath => "analyze_full_path",
            Phase::Finalized => "finalized",
        }
    }

    pub fn parse(s: &str) -> Option<Phase> {
        Self::ALL.into_iter().find(|p| p.as_str() == s)
    }

    pub fn next(self) -> Option<Phase> {
        Self::ALL.get(self as usize + 1).copied()
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─── scan.json ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scan {
    pub schema: u32,
    pub scan_id: String,
    pub target_key: String,
    pub root: PathBuf,
    #[serde(default)]
    pub remote: Option<String>,
    pub revision: String,
    pub dirty: bool,
    #[serde(default)]
    pub snapshot_digest: Option<String>,
    pub domain: String,
    pub mode: String,
    #[serde(default)]
    pub base_scan_id: Option<String>,
    pub phase: Phase,
    pub writer: Writer,
    #[serde(default)]
    pub user_requests: Vec<UserRequest>,
    #[serde(default)]
    pub policy: Policy,
    pub templates: TemplatesMeta,
    /// 开始时生效的域包（这一版之前建的扫描没有，读出来按三层重新解析）。
    #[serde(default)]
    pub pack: PackMeta,
    #[serde(default)]
    pub sealed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Writer {
    pub session_id: String,
    pub pid: u32,
    pub since: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRequest {
    pub text: String,
    pub at: String,
    /// prompt | append | answer
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplatesMeta {
    pub digest: String,
    #[serde(default)]
    pub frozen: bool,
}

/// 扫描开始时生效的域包：哪个域、规则来自哪一层、哪些层叠了文件、规则摘要。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PackMeta {
    pub domain: String,
    #[serde(default)]
    pub description: String,
    /// 规则和方法文件（`pack.json` + `method/**`）的摘要。
    #[serde(default)]
    pub digest: String,
    /// `embedded` | `user` | `project`：规则来自哪一层。
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub overlays: Vec<Overlay>,
}

/// 某一层往域包里叠了哪些文件。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
    /// `user` | `project`。
    pub level: String,
    pub files: Vec<String>,
}

// ─── state.json ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub schema: u32,
    #[serde(default)]
    pub next_section: u32,
    #[serde(default)]
    pub next_finding: u32,
    #[serde(default)]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub exclusions: Vec<Exclusion>,
}

impl State {
    pub fn section(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }
    pub fn finding(&self, id: &str) -> Option<&Finding> {
        self.findings.iter().find(|f| f.id == id)
    }
    pub fn section_index(&self, id: &str) -> Option<usize> {
        self.sections.iter().position(|s| s.id == id)
    }
    pub fn finding_index(&self, id: &str) -> Option<usize> {
        self.findings.iter().position(|f| f.id == id)
    }
    /// 还起作用的板块：没并入、没废除。
    pub fn active_sections(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(|s| s.merged_into.is_none() && s.abolished.is_none())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub paths: Vec<String>,
    pub questions: Vec<String>,
    #[serde(default)]
    pub disposition: Option<String>,
    #[serde(default)]
    pub disposition_reason: Option<String>,
    #[serde(default)]
    pub reopened: bool,
    #[serde(default)]
    pub reopened_at: Option<String>,
    #[serde(default)]
    pub merged_into: Option<String>,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub abolished: Option<AbolishInfo>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
    pub created_at: String,
}

impl Section {
    pub fn is_live(&self) -> bool {
        self.merged_into.is_none() && self.abolished.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbolishInfo {
    pub reason: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exclusion {
    pub path: String,
    pub reason: String,
    #[serde(default)]
    pub added_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: String,
    pub section_id: String,
    pub kind: String,
    pub identity: Identity,
    pub fingerprint: String,
    pub anchors: Vec<String>,
    /// 最近一次解析的结果：每个定位引用了哪些行、内容摘要是什么。
    #[serde(default)]
    pub anchor_refs: Vec<AnchorRef>,
    pub status: FindingStatus,
    pub fields: Map<String, Value>,
    #[serde(default)]
    pub paths: Vec<PathRecord>,
    #[serde(default)]
    pub retracted_paths: Vec<RetractedPath>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
    #[serde(default)]
    pub retracted: Option<RetractInfo>,
    pub created_at: String,
    pub updated_at: String,
}

impl Finding {
    /// 还起作用的断言：没撤回。
    pub fn is_effective(&self) -> bool {
        self.retracted.is_none()
    }
    pub fn verdict(&self) -> Option<&str> {
        self.fields.get("verdict").and_then(Value::as_str)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub anchor: String,
    #[serde(default)]
    pub instance: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnchorRef {
    pub anchor: String,
    /// start | end | ref
    pub role: String,
    pub line: u32,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    #[default]
    Candidate,
    Validated,
    Refuted,
    Deferred,
    Retracted,
}

impl FindingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingStatus::Candidate => "candidate",
            FindingStatus::Validated => "validated",
            FindingStatus::Refuted => "refuted",
            FindingStatus::Deferred => "deferred",
            FindingStatus::Retracted => "retracted",
        }
    }
    /// 终态：证实、推翻、搁置。
    pub fn is_terminal(self) -> bool {
        matches!(self, FindingStatus::Validated | FindingStatus::Refuted | FindingStatus::Deferred)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathRecord {
    pub steps: Vec<Step>,
    #[serde(default)]
    pub prerequisites: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
    #[serde(default)]
    pub attached_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub role: String,
    #[serde(default)]
    pub anchor: Option<String>,
    #[serde(default)]
    pub gap: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetractedPath {
    #[serde(flatten)]
    pub path: PathRecord,
    pub reason: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetractInfo {
    pub reason: String,
    pub at: String,
}

/// 记录警告：随记录保存，进下一阶段前必须清除。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    pub code: String,
    #[serde(default)]
    pub field: String,
    pub message: String,
    #[serde(default)]
    pub stage: Option<Phase>,
}

// ─── progress.json ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    #[serde(default)]
    pub active_sections: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub waiting_for_decision: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

// ─── 缺口 ────────────────────────────────────────────────────────────────────

/// 关卡缺口：固定句式加数据。refs 是涉及的记录标识，agent 可以直接按标识取。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

// ─── 板块状态 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionStatus {
    Planned,
    Investigating,
    Reported,
    FollowUp,
    Excluded,
    NoIssue,
    NotApplicable,
    Missing,
    Merged,
    Abolished,
}

impl SectionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SectionStatus::Planned => "planned",
            SectionStatus::Investigating => "investigating",
            SectionStatus::Reported => "reported",
            SectionStatus::FollowUp => "follow_up",
            SectionStatus::Excluded => "excluded",
            SectionStatus::NoIssue => "no_issue",
            SectionStatus::NotApplicable => "not_applicable",
            SectionStatus::Missing => "missing",
            SectionStatus::Merged => "merged",
            SectionStatus::Abolished => "abolished",
        }
    }
}
