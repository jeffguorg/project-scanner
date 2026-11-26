//! 状态由内核从记录推导，agent 不能直接写。

use crate::model::{Finding, FindingStatus, Section, SectionStatus, State};

pub fn finding_status(f: &Finding) -> FindingStatus {
    if f.retracted.is_some() {
        return FindingStatus::Retracted;
    }
    match f.verdict() {
        Some("validated") => FindingStatus::Validated,
        Some("refuted") => FindingStatus::Refuted,
        Some("deferred") => FindingStatus::Deferred,
        _ => FindingStatus::Candidate,
    }
}

pub fn section_status(s: &Section, state: &State) -> SectionStatus {
    if s.merged_into.is_some() {
        return SectionStatus::Merged;
    }
    if s.abolished.is_some() {
        return SectionStatus::Abolished;
    }
    let effective: Vec<&Finding> = state.findings.iter().filter(|f| f.section_id == s.id && f.is_effective()).collect();
    if effective.iter().any(|f| f.status == FindingStatus::Validated) && !s.reopened {
        return SectionStatus::Reported;
    }
    if s.reopened || effective.iter().any(|f| f.status == FindingStatus::Candidate) {
        return SectionStatus::Investigating;
    }
    if effective.iter().any(|f| f.status == FindingStatus::Deferred) {
        return SectionStatus::FollowUp;
    }
    if effective.iter().any(|f| f.status == FindingStatus::Refuted) {
        return SectionStatus::Excluded;
    }
    match s.disposition.as_deref() {
        Some("no_issue") => SectionStatus::NoIssue,
        Some("not_applicable") => SectionStatus::NotApplicable,
        Some("deferred") => SectionStatus::FollowUp,
        _ => SectionStatus::Planned,
    }
}

/// 每次写入后重算全部断言状态。
pub fn recompute_findings(state: &mut State) {
    let statuses: Vec<FindingStatus> = state.findings.iter().map(finding_status).collect();
    for (f, st) in state.findings.iter_mut().zip(statuses) {
        f.status = st;
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub sections: std::collections::BTreeMap<String, usize>,
    pub findings: std::collections::BTreeMap<String, usize>,
    pub with_warnings: usize,
    pub paths_required: usize,
    pub paths_complete: usize,
    pub paths_incomplete: usize,
    pub retracted_findings: usize,
}

pub fn counts(state: &State, pack: &crate::pack::Pack) -> Counts {
    let mut c = Counts::default();
    for s in &state.sections {
        let st = section_status(s, state);
        *c.sections.entry(st.as_str().to_string()).or_default() += 1;
    }
    for f in &state.findings {
        *c.findings.entry(f.status.as_str().to_string()).or_default() += 1;
        if f.is_effective() && !f.warnings.is_empty() {
            c.with_warnings += 1;
        }
        if f.is_effective() && f.status == FindingStatus::Validated && pack.path_required_kinds.contains(&f.kind) {
            c.paths_required += 1;
            if !f.paths.is_empty() && f.paths.iter().all(|p| super::gates::path_complete(p, pack).is_ok()) {
                c.paths_complete += 1;
            } else {
                c.paths_incomplete += 1;
            }
        }
    }
    c.retracted_findings = state.findings.iter().filter(|f| !f.is_effective()).count();
    c
}
