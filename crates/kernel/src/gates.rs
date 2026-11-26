//! 关卡：阶段之间的放行条件。`evaluate(target)` 返回缺口；每次关卡前重新解析全部定位。

use crate::anchor;

use crate::inventory::{self, Inventory};
use crate::model::*;
use crate::pack::Pack;
use crate::status;
use crate::templates::TemplateSet;
use crate::text;
use serde_json::{json, Map, Value};

pub struct GateEnv<'a> {
    pub scan: &'a Scan,
    pub inventory: &'a Inventory,
    pub pack: &'a Pack,
    pub templates: &'a TemplateSet,
    /// 策略文件原文，渲染进报告。
    pub policy_text: Option<&'a str>,
}

// ─── 绑定对象 ────────────────────────────────────────────────────────────────

pub fn section_obj(s: &Section) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("label".into(), json!(s.label));
    m.insert("kind".into(), json!(s.kind));
    m.insert("paths".into(), json!(s.paths));
    m.insert("questions".into(), json!(s.questions));
    m.insert("disposition".into(), json!(s.disposition));
    m.insert("dispositionReason".into(), json!(s.disposition_reason));
    m
}

pub fn finding_obj(f: &Finding) -> Map<String, Value> {
    let mut m = f.fields.clone();
    m.insert("anchors".into(), json!(f.anchors));
    m.insert("kind".into(), json!(f.kind));
    m
}

pub fn path_obj(p: &PathRecord) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("steps".into(), json!(p.steps));
    m.insert("prerequisites".into(), json!(p.prerequisites));
    m
}

pub fn exclusion_obj(path: &str, reason: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("path".into(), json!(path));
    m.insert("reason".into(), json!(reason));
    m
}

// ─── 路径 ────────────────────────────────────────────────────────────────────

/// 路径完整：至少一个起点、一个结果，都有定位；每步恰好是定位或缺口，缺口写明理由。
pub fn path_complete(p: &PathRecord, pack: &Pack) -> Result<(), String> {
    let has = |roles: &[String]| {
        p.steps.iter().any(|s| roles.contains(&s.role) && s.anchor.is_some())
    };
    if !has(&pack.origin_roles) {
        return Err(format!("缺起点（{}）的定位", pack.origin_roles.join("/")));
    }
    if !has(&pack.outcome_roles) {
        return Err(format!("缺结果（{}）的定位", pack.outcome_roles.join("/")));
    }
    Ok(())
}

// ─── 刷新 ────────────────────────────────────────────────────────────────────

fn stale_warning(field: String, a: &str) -> Warning {
    Warning { code: "W_ANCHOR_STALE".into(), field, message: format!("定位过期：{a}"), stage: None }
}

/// 重新解析一条断言的全部定位；摘要不一致或解析不了就是过期。旧摘要保留，直到 agent 重新提交定位。
fn refresh_finding(scan: &Scan, f: &mut Finding, templates: &TemplateSet, up_to: Phase) {
    if !f.is_effective() {
        f.warnings.clear();
        return;
    }
    let mut stale: Vec<Warning> = Vec::new();
    let mut new_refs: Vec<AnchorRef> = Vec::new();
    for (i, a) in f.anchors.iter().enumerate() {
        let old: Vec<AnchorRef> = f.anchor_refs.iter().filter(|r| &r.anchor == a).cloned().collect();
        match anchor::parse(a).and_then(|p| anchor::resolve(&scan.root, a, &p)) {
            Ok(r) => {
                if anchor::same_refs(&old, &r.refs) || old.is_empty() {
                    new_refs.extend(r.refs);
                } else {
                    new_refs.extend(old);
                    stale.push(stale_warning(format!("anchors[{i}]"), a));
                }
            }
            Err(_) => {
                if old.is_empty() {
                    stale.push(stale_warning(format!("anchors[{i}]"), a));
                } else {
                    new_refs.extend(old);
                    stale.push(stale_warning(format!("anchors[{i}]"), a));
                }
            }
        }
    }
    f.anchor_refs = new_refs;
    for (pi, p) in f.paths.iter_mut().enumerate() {
        for (si, s) in p.steps.iter_mut().enumerate() {
            if let Some(a) = &s.anchor {
                if anchor::parse(a).and_then(|q| anchor::resolve(&scan.root, a, &q)).is_err() {
                    stale.push(stale_warning(format!("paths[{pi}].steps[{si}]"), a));
                }
            }
        }
        let mut w = templates.bind("path", None, &path_obj(p), up_to.min(Phase::AnalyzeFullPath));
        w.extend(stale.iter().filter(|x| x.field.starts_with(&format!("paths[{pi}]"))).cloned());
        p.warnings = w;
    }
    let cap = Phase::AnalyzeFullPath;
    let mut warnings = templates.bind("finding", Some(&f.kind), &finding_obj(f), up_to.min(cap));
    warnings.extend(stale.into_iter().filter(|w| w.field.starts_with("anchors")));
    f.warnings = warnings;
}

/// 关卡前：重新解析全部定位、重绑全部记录（到 up_to 阶段的片段），警告写回记录。
pub fn refresh_all(scan: &Scan, state: &mut State, templates: &TemplateSet, up_to: Phase) {
    status::recompute_findings(state);
    for f in state.findings.iter_mut() {
        refresh_finding(scan, f, templates, up_to);
    }
    for s in state.sections.iter_mut() {
        if !s.is_live() {
            s.warnings.clear();
            continue;
        }
        s.warnings = templates.bind("section", Some(&s.kind), &section_obj(s), Phase::Planning);
    }
}

/// 只刷新本次写到的记录（记录时）。
pub fn refresh_touched(scan: &Scan, state: &mut State, templates: &TemplateSet, up_to: Phase, ids: &[String]) {
    status::recompute_findings(state);
    let touched: Vec<String> = state.findings.iter().filter(|f| ids.iter().any(|i| i == &f.id)).map(|f| f.id.clone()).collect();
    for f in state.findings.iter_mut() {
        if touched.contains(&f.id) {
            refresh_finding(scan, f, templates, up_to);
        }
    }
    let touched_sections: Vec<String> = state
        .findings
        .iter()
        .filter(|f| touched.contains(&f.id))
        .map(|f| f.section_id.clone())
        .chain(ids.iter().filter(|i| i.starts_with("s-")).cloned())
        .collect();
    for s in state.sections.iter_mut() {
        if touched_sections.contains(&s.id) && s.is_live() {
            s.warnings = templates.bind("section", Some(&s.kind), &section_obj(s), Phase::Planning);
        }
    }
}

// ─── 关卡 ────────────────────────────────────────────────────────────────────

pub fn output_counts(templates: &TemplateSet) -> (usize, usize) {
    let all = templates.outputs().iter().filter(|o| matches!(&o.meta, crate::templates::Meta::Output { select, .. } if select == "all")).count();
    let section = templates.outputs().iter().filter(|o| matches!(&o.meta, crate::templates::Meta::Output { select, .. } if select == "section")).count();
    (all, section)
}

pub fn evaluate(target: Phase, state: &State, env: &GateEnv) -> Vec<Gap> {
    let mut gaps = Vec::new();
    if target >= Phase::Discovery {
        plan_gaps(state, env, &mut gaps);
    }
    if target >= Phase::Validation {
        validation_gaps(state, env, target, &mut gaps);
    }
    if target >= Phase::Finalized {
        finalize_gaps(state, env, &mut gaps);
    }
    gaps
}

fn plan_gaps(state: &State, env: &GateEnv, gaps: &mut Vec<Gap>) {
    let active: Vec<&Section> = state.active_sections().collect();
    if active.is_empty() {
        gaps.push(text::g_plan_empty());
        return;
    }
    for s in &active {
        if s.paths.is_empty() || s.questions.is_empty() {
            gaps.push(text::g_section_no_scope(&s.id, &s.label));
        }
        if !s.warnings.is_empty() {
            gaps.push(text::g_findings_with_warnings(&[(s.id.clone(), s.warnings.clone())]));
        }
    }
    let unc = inventory::uncovered(env.inventory, state.active_sections(), &state.exclusions);
    if !unc.is_empty() {
        let sample: Vec<String> = unc.iter().take(20).cloned().collect();
        gaps.push(text::g_section_uncovered_files(unc.len(), &sample));
    }
    let (all, section) = output_counts(env.templates);
    if all + section == 0 {
        for s in &active {
            gaps.push(text::g_section_no_output(&s.id, &s.label));
        }
    }
}

fn validation_gaps(state: &State, env: &GateEnv, target: Phase, gaps: &mut Vec<Gap>) {
    let mut investigating = Vec::new();
    for s in state.active_sections() {
        let effective: Vec<&Finding> = state.findings.iter().filter(|f| f.section_id == s.id && f.is_effective()).collect();
        if effective.is_empty() && s.disposition.is_none() {
            gaps.push(text::g_section_no_disposition(&s.id, &s.label));
        }
        if status::section_status(s, state) == SectionStatus::Investigating {
            investigating.push((s.id.clone(), s.label.clone(), s.reopened));
        }
    }
    if !investigating.is_empty() {
        gaps.push(text::g_section_investigating(&investigating));
    }
    let mut warned: Vec<(String, Vec<Warning>)> = Vec::new();
    for f in state.findings.iter().filter(|f| f.is_effective()) {
        let mut w = templates_warnings(env, f, target);
        if target >= Phase::Finalized {
            for (i, p) in f.paths.iter().enumerate() {
                let mut pw = env.templates.bind("path", None, &path_obj(p), Phase::AnalyzeFullPath);
                pw.extend(p.warnings.iter().filter(|x| x.code == "W_ANCHOR_STALE").cloned());
                for x in pw.iter_mut() {
                    x.field = format!("paths[{i}].{}", x.field);
                }
                w.extend(pw);
            }
        }
        if !w.is_empty() {
            warned.push((f.id.clone(), w));
        }
    }
    if !warned.is_empty() {
        gaps.push(text::g_findings_with_warnings(&warned));
    }
    if target >= Phase::AnalyzeFullPath {
        let no_verdict: Vec<String> = state
            .findings
            .iter()
            .filter(|f| f.is_effective() && !f.status.is_terminal())
            .map(|f| f.id.clone())
            .collect();
        if !no_verdict.is_empty() {
            gaps.push(text::g_finding_no_verdict(&no_verdict));
        }
    }
    // 排除的理由由片段声明时，也要绑
    for e in &state.exclusions {
        let w = env.templates.bind("exclusion", None, &exclusion_obj(&e.path, &e.reason), Phase::Planning);
        if !w.is_empty() {
            gaps.push(text::g_findings_with_warnings(&[(e.path.clone(), w)]));
        }
    }
}

fn templates_warnings(env: &GateEnv, f: &Finding, target: Phase) -> Vec<Warning> {
    let mut w = env.templates.bind("finding", Some(&f.kind), &finding_obj(f), target.min(Phase::AnalyzeFullPath));
    w.extend(f.warnings.iter().filter(|x| x.code == "W_ANCHOR_STALE").cloned());
    w
}

fn finalize_gaps(state: &State, env: &GateEnv, gaps: &mut Vec<Gap>) {
    for f in state.findings.iter().filter(|f| f.is_effective() && f.status == FindingStatus::Validated) {
        if env.pack.path_required_kinds.contains(&f.kind) {
            if f.paths.is_empty() {
                gaps.push(text::g_path_missing(&f.id, &f.kind));
                continue;
            }
        }
        for (i, p) in f.paths.iter().enumerate() {
            if let Err(what) = path_complete(p, env.pack) {
                gaps.push(text::g_path_incomplete(&f.id, i, &what));
            }
            for s in &p.steps {
                if env.pack.origin_roles.contains(&s.role) {
                    if let Some(a) = &s.anchor {
                        let in_scope = anchor::parse(a).map(|x| env.inventory.contains_path(&x.relpath)).unwrap_or(false);
                        if !in_scope {
                            let rel = anchor::parse(a).map(|x| x.relpath).unwrap_or_else(|_| a.clone());
                            gaps.push(text::g_path_origin_out_of_scope(&f.id, i, &rel));
                        }
                    }
                }
            }
        }
    }
    let (all, section) = output_counts(env.templates);
    for s in state.active_sections() {
        let n = all + section;
        if n != 1 {
            gaps.push(text::g_orphan_record(&s.id, &s.label, n));
        }
    }
    // 渲染检查：哪一关都做，模板坏了越早发现越好。
    let input = crate::render::RenderInput { scan: env.scan, state, pack: env.pack, policy_text: env.policy_text };
    if let Err(e) = crate::render::render_all(env.templates, &input) {
        gaps.push(text::g_render_failed("*", &e));
    }
}

/// 下一关的缺口（只算不写）。给 scan_context 和 scans show 用。
pub fn preview_next(scan: &Scan, state: &State, env: &GateEnv) -> (Phase, Vec<Gap>) {
    let target = scan.phase.next().unwrap_or(Phase::Finalized);
    let mut copy = state.clone();
    refresh_all(scan, &mut copy, env.templates, target);
    (target, evaluate(target, &copy, env))
}

