//! scan_record：一批操作 = 一个检查点。先全部做结构校验（写者、阶段权限、定位、身份），
//! 任一失败整批不写；通过后应用、绑定片段、评估下一关并可能推进。

use crate::anchor;
use crate::error::{KError, KResult};
use crate::gates::{self, GateEnv};
use crate::inventory::Inventory;
use crate::model::*;
use crate::pack::Pack;
use crate::store::ScanStore;
use crate::templates::TemplateSet;
use crate::text;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub enum Op {
    SectionCreate { label: String, kind: String, paths: Vec<String>, questions: Vec<String> },
    SectionUpdate { id: String, label: Option<String>, paths: Option<Vec<String>>, questions: Option<Vec<String>> },
    SectionMerge { id: String, into: String },
    SectionAbolish { id: String, reason: String },
    FindingCreate { section_id: String, kind: String, identity: Identity, anchors: Vec<String>, fields: Map<String, Value> },
    FindingUpdate { id: String, anchors: Option<Vec<String>>, fields: Map<String, Value> },
    FindingRetract { id: String, reason: String },
    DispositionSet { section_id: String, disposition: String, reason: String },
    VerdictAttach { id: String, fields: Map<String, Value> },
    EvidenceAppend { id: String, anchors: Vec<String> },
    PathAttach { id: String, steps: Vec<Step>, prerequisites: Vec<String> },
    PathUpdate { id: String, index: usize, steps: Vec<Step>, prerequisites: Vec<String> },
    PathRetract { id: String, index: usize, reason: String },
    ExclusionAdd { path: String, reason: String },
    ExclusionRemove { path: String },
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::SectionCreate { .. } => "section.create",
            Op::SectionUpdate { .. } => "section.update",
            Op::SectionMerge { .. } => "section.merge",
            Op::SectionAbolish { .. } => "section.abolish",
            Op::FindingCreate { .. } => "finding.create",
            Op::FindingUpdate { .. } => "finding.update",
            Op::FindingRetract { .. } => "finding.retract",
            Op::DispositionSet { .. } => "disposition.set",
            Op::VerdictAttach { .. } => "verdict.attach",
            Op::EvidenceAppend { .. } => "evidence.append",
            Op::PathAttach { .. } => "path.attach",
            Op::PathUpdate { .. } => "path.update",
            Op::PathRetract { .. } => "path.retract",
            Op::ExclusionAdd { .. } => "exclusion.add",
            Op::ExclusionRemove { .. } => "exclusion.remove",
        }
    }
}

// ─── 解析 ────────────────────────────────────────────────────────────────────

struct OpReader<'a> {
    v: &'a Map<String, Value>,
    i: usize,
    op: &'a str,
}

impl<'a> OpReader<'a> {
    fn err(&self, what: &str) -> KError {
        KError::invalid_input(format!("第 {} 个操作（{}）：{what}", self.i + 1, self.op))
    }
    fn s(&self, key: &str) -> KResult<String> {
        match self.v.get(key) {
            Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
            Some(Value::String(_)) => Err(self.err(&format!("{key} 不能是空白"))),
            _ => Err(self.err(&format!("缺少字符串字段 {key}"))),
        }
    }
    fn opt_s(&self, key: &str) -> KResult<Option<String>> {
        match self.v.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            _ => Err(self.err(&format!("{key} 应为字符串"))),
        }
    }
    fn strings(&self, key: &str, required: bool) -> KResult<Option<Vec<String>>> {
        match self.v.get(key) {
            None | Some(Value::Null) if !required => Ok(None),
            Some(Value::Array(a)) => {
                let mut out = Vec::new();
                for x in a {
                    match x {
                        Value::String(s) if !s.trim().is_empty() => out.push(s.clone()),
                        _ => return Err(self.err(&format!("{key} 的每一项应为非空字符串"))),
                    }
                }
                Ok(Some(out))
            }
            _ if required => Err(self.err(&format!("缺少列表字段 {key}"))),
            _ => Err(self.err(&format!("{key} 应为字符串列表"))),
        }
    }
    fn obj(&self, key: &str, required: bool) -> KResult<Option<Map<String, Value>>> {
        match self.v.get(key) {
            None | Some(Value::Null) if !required => Ok(None),
            Some(Value::Object(m)) => Ok(Some(m.clone())),
            _ if required => Err(self.err(&format!("缺少对象字段 {key}"))),
            _ => Err(self.err(&format!("{key} 应为对象"))),
        }
    }
    fn idx(&self, key: &str) -> KResult<usize> {
        match self.v.get(key) {
            Some(Value::Number(n)) if n.as_u64().is_some() => Ok(n.as_u64().unwrap() as usize),
            _ => Err(self.err(&format!("{key} 应为非负整数"))),
        }
    }
}

fn identity(r: &OpReader<'_>) -> KResult<Identity> {
    let m = r.obj("identity", true)?.ok_or_else(|| r.err("缺少 identity"))?;
    let anchor = match m.get("anchor") {
        Some(Value::String(s)) => s.clone(),
        _ => return Err(r.err("identity.anchor 应为字符串")),
    };
    let instance = match m.get("instance") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        _ => return Err(r.err("identity.instance 应为非空字符串或省略")),
    };
    Ok(Identity { anchor, instance })
}

fn steps(r: &OpReader<'_>) -> KResult<Vec<Step>> {
    let arr = match r.v.get("steps") {
        Some(Value::Array(a)) => a,
        _ => return Err(r.err("缺少 steps 列表")),
    };
    let mut out = Vec::new();
    for (j, s) in arr.iter().enumerate() {
        let Value::Object(m) = s else {
            return Err(r.err(&format!("steps[{j}] 应为对象")));
        };
        let role = match m.get("role") {
            Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
            _ => return Err(r.err(&format!("steps[{j}].role 应为非空字符串"))),
        };
        let anchor = match m.get("anchor") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
            _ => return Err(r.err(&format!("steps[{j}].anchor 应为非空字符串或省略"))),
        };
        let gap = match m.get("gap") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
            _ => return Err(r.err(&format!("steps[{j}].gap 应为非空字符串或省略"))),
        };
        let note = match m.get("note") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            _ => return Err(r.err(&format!("steps[{j}].note 应为字符串"))),
        };
        match (&anchor, &gap) {
            (Some(_), Some(_)) => return Err(r.err(&format!("steps[{j}]：anchor 和 gap 只能给一个"))),
            (None, None) => return Err(r.err(&format!("steps[{j}]：要有 anchor（定位）或 gap（缺口及理由）"))),
            _ => {}
        }
        out.push(Step { role, anchor, gap, note });
    }
    Ok(out)
}

pub fn parse_op(i: usize, v: &Value) -> KResult<Op> {
    let Value::Object(m) = v else {
        return Err(KError::invalid_input(format!("第 {} 个操作不是对象", i + 1)));
    };
    let op = match m.get("op") {
        Some(Value::String(s)) => s.clone(),
        _ => return Err(KError::invalid_input(format!("第 {} 个操作缺少 op", i + 1))),
    };
    let r = OpReader { v: m, i, op: &op };
    Ok(match op.as_str() {
        "section.create" => Op::SectionCreate {
            label: r.s("label")?,
            kind: r.s("kind")?,
            paths: r.strings("paths", true)?.unwrap(),
            questions: r.strings("questions", true)?.unwrap(),
        },
        "section.update" => Op::SectionUpdate {
            id: r.s("id")?,
            label: r.opt_s("label")?,
            paths: r.strings("paths", false)?,
            questions: r.strings("questions", false)?,
        },
        "section.merge" => Op::SectionMerge { id: r.s("id")?, into: r.s("into")? },
        "section.abolish" => Op::SectionAbolish { id: r.s("id")?, reason: r.s("reason")? },
        "finding.create" => Op::FindingCreate {
            section_id: r.s("sectionId")?,
            kind: r.s("kind")?,
            identity: identity(&r)?,
            anchors: r.strings("anchors", true)?.unwrap(),
            fields: r.obj("fields", false)?.unwrap_or_default(),
        },
        "finding.update" => Op::FindingUpdate {
            id: r.s("id")?,
            anchors: r.strings("anchors", false)?,
            fields: r.obj("fields", false)?.unwrap_or_default(),
        },
        "finding.retract" => Op::FindingRetract { id: r.s("id")?, reason: r.s("reason")? },
        "disposition.set" => Op::DispositionSet {
            section_id: r.s("sectionId")?,
            disposition: r.s("disposition")?,
            reason: r.s("reason")?,
        },
        "verdict.attach" => Op::VerdictAttach { id: r.s("id")?, fields: r.obj("fields", true)?.unwrap() },
        "evidence.append" => Op::EvidenceAppend { id: r.s("id")?, anchors: r.strings("anchors", true)?.unwrap() },
        "path.attach" => Op::PathAttach { id: r.s("id")?, steps: steps(&r)?, prerequisites: r.strings("prerequisites", false)?.unwrap_or_default() },
        "path.update" => Op::PathUpdate {
            id: r.s("id")?,
            index: r.idx("index")?,
            steps: steps(&r)?,
            prerequisites: r.strings("prerequisites", false)?.unwrap_or_default(),
        },
        "path.retract" => Op::PathRetract { id: r.s("id")?, index: r.idx("index")?, reason: r.s("reason")? },
        "exclusion.add" => Op::ExclusionAdd { path: r.s("path")?, reason: r.s("reason")? },
        "exclusion.remove" => Op::ExclusionRemove { path: r.s("path")? },
        other => {
            return Err(KError::invalid_input(format!(
                "第 {} 个操作的 op {other:?} 不认识。可用：section.create/update/merge/abolish、finding.create/update/retract、disposition.set、verdict.attach、evidence.append、path.attach/update/retract、exclusion.add/remove",
                i + 1
            )))
        }
    })
}

// ─── 结构校验 ────────────────────────────────────────────────────────────────

pub struct Ctx<'a> {
    pub scan: &'a Scan,
    pub inventory: &'a Inventory,
    pub pack: &'a Pack,
    pub now: &'a str,
}

fn check_anchor(ctx: &Ctx<'_>, s: &str) -> KResult<anchor::Resolved> {
    let a = anchor::parse(s).map_err(|r| KError::new("E_ANCHOR_INVALID", text::e_anchor_invalid(&r)))?;
    if a.commit != ctx.scan.revision {
        return Err(KError::new("E_ANCHOR_REVISION_MISMATCH", text::e_anchor_revision_mismatch(s, "修订")));
    }
    if a.dirty != ctx.scan.dirty {
        return Err(KError::new("E_ANCHOR_REVISION_MISMATCH", text::e_anchor_revision_mismatch(s, "+dirty 标记")));
    }
    if !ctx.inventory.contains_path(&a.relpath) {
        return Err(KError::new("E_ANCHOR_OUT_OF_SCOPE", text::e_anchor_out_of_scope(s)));
    }
    anchor::resolve(&ctx.scan.root, s, &a).map_err(|r| KError::new("E_ANCHOR_INVALID", text::e_anchor_invalid(&format!("{r}（{s}）"))))
}

fn norm_rel(p: &str) -> String {
    p.trim().trim_start_matches("./").trim_end_matches('/').to_string()
}

fn check_paths(ctx: &Ctx<'_>, paths: &[String]) -> KResult<Vec<String>> {
    let mut out = Vec::new();
    for p in paths {
        let n = norm_rel(p);
        if n.is_empty() || n.starts_with('/') || n.split('/').any(|s| s == "..") {
            return Err(KError::invalid_input(format!("板块范围 {p:?} 应是相对根目录的路径")));
        }
        if !ctx.inventory.contains_path(&n) {
            return Err(KError::new("E_SECTION_PATH_UNKNOWN", format!("范围 {p:?} 不在清点范围内：文件不存在、已默认排除或不是文本文件")));
        }
        out.push(n);
    }
    Ok(out)
}

fn check_steps(ctx: &Ctx<'_>, steps: &[Step]) -> KResult<()> {
    for (j, s) in steps.iter().enumerate() {
        if !ctx.pack.path_roles.contains(&s.role) {
            return Err(KError::invalid_input(format!("steps[{j}].role {:?} 不在域的角色里：{}", s.role, ctx.pack.path_roles.join("、"))));
        }
        if let Some(a) = &s.anchor {
            check_anchor(ctx, a)?;
        }
    }
    Ok(())
}

fn effective_in(state: &State, section_id: &str) -> usize {
    state.findings.iter().filter(|f| f.section_id == section_id && f.is_effective()).count()
}

fn live_section(state: &State, id: &str) -> KResult<usize> {
    let i = state.section_index(id).ok_or_else(|| KError::new("E_UNKNOWN_RECORD", text::e_unknown_record(id)))?;
    if !state.sections[i].is_live() {
        return Err(KError::new("E_UNKNOWN_RECORD", format!("板块 {} 已{}，不能再写", id, if state.sections[i].merged_into.is_some() { "并入别的板块" } else { "废除" })));
    }
    Ok(i)
}

fn live_finding(state: &State, id: &str) -> KResult<usize> {
    let i = state.finding_index(id).ok_or_else(|| KError::new("E_UNKNOWN_RECORD", text::e_unknown_record(id)))?;
    if !state.findings[i].is_effective() {
        return Err(KError::invalid_input(format!("断言 {id} 已撤回，不能再写")));
    }
    Ok(i)
}

fn allowed(phase: Phase, op: &Op, reopened: &BTreeSet<String>, state: &State) -> bool {
    let sec_reopened = |sid: &str| reopened.contains(sid);
    match phase {
        Phase::Preflight | Phase::Finalized => false,
        Phase::Planning => matches!(op, Op::SectionCreate { .. } | Op::SectionUpdate { .. } | Op::ExclusionAdd { .. } | Op::ExclusionRemove { .. }),
        Phase::Discovery | Phase::Validation | Phase::AnalyzeFullPath => {
            let base = matches!(op, Op::SectionCreate { .. } | Op::SectionUpdate { .. } | Op::SectionMerge { .. } | Op::SectionAbolish { .. } | Op::FindingRetract { .. } | Op::ExclusionAdd { .. } | Op::ExclusionRemove { .. });
            let sec_of = |id: &str| state.finding(id).map(|f| f.section_id.clone()).unwrap_or_default();
            base || match op {
                Op::FindingCreate { section_id, .. } => phase == Phase::Discovery || sec_reopened(section_id),
                Op::FindingUpdate { id, .. } | Op::VerdictAttach { id, .. } => phase != Phase::AnalyzeFullPath || sec_reopened(&sec_of(id)),
                Op::DispositionSet { .. } => true,
                Op::EvidenceAppend { .. } => true,
                Op::PathAttach { .. } | Op::PathUpdate { .. } | Op::PathRetract { .. } => phase == Phase::AnalyzeFullPath,
                _ => false,
            }
        }
    }
}

// ─── 应用 ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub written: bool,
    pub duplicate: bool,
    pub ids: Vec<String>,
    pub warnings: Vec<RecordWarnings>,
    pub phase: Phase,
    pub advanced_from: Option<Phase>,
    pub gaps: Vec<Gap>,
    pub checkpoint: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordWarnings {
    pub record_id: String,
    pub warnings: Vec<Warning>,
}

fn commit(work: &mut State, ids: &[String]) {
    for s in work.sections.iter_mut() {
        if s.reopened {
            let touched = ids.iter().any(|i| i == &s.id) || work.findings.iter().any(|f| s.id == f.section_id && ids.contains(&f.id));
            if touched {
                s.reopened = false;
                s.reopened_at = None;
            }
        }
    }
}

/// 应用一批操作。调用方持有排他锁。成功时 state 和 scan 已被更新并写盘。
pub fn apply(
    store: &ScanStore,
    scan: &mut Scan,
    state: &mut State,
    templates: &TemplateSet,
    inventory: &Inventory,
    pack: &Pack,
    session_id: &str,
    ops: &[Value],
    now: &str,
) -> KResult<Outcome> {
    crate::store::writer_check(scan, session_id)?;
    if ops.is_empty() {
        return Err(KError::invalid_input("ops 为空"));
    }
    let parsed: Vec<Op> = ops.iter().enumerate().map(|(i, v)| parse_op(i, v)).collect::<KResult<_>>()?;
    let input_digest = crate::digest_json(&Value::Array(ops.to_vec()));

    // 重试安全：同一内容重复提交不产生新检查点。
    if let Ok((head_digest, head)) = read_head(store) {
        if head_digest == input_digest {
            // 检查点文件是 { digest, at, writer, input, result }，结果是其中的 result。
            let cp: Checkpoint = crate::store::ScanStore::read(&head).map_err(crate::store::io_err)?;
            let mut o = cp.result;
            o.duplicate = true;
            o.written = true;
            return Ok(o);
        }
    }

    let ctx = Ctx { scan, inventory, pack, now };
    let mut work = state.clone();
    let reopened: BTreeSet<String> = work.sections.iter().filter(|s| s.reopened && s.is_live()).map(|s| s.id.clone()).collect();
    let mut ids: Vec<String> = Vec::new();

    for op in &parsed {
        if !allowed(scan.phase, op, &reopened, &work) {
            return Err(KError::new("E_PHASE_WRITE_FORBIDDEN", text::e_phase_write_forbidden(scan.phase, op.name())).with_data(json!({ "op": op.name() })));
        }
        match op {
            Op::SectionCreate { label, kind, paths, questions } => {
                if !pack.section_kinds.contains(kind) {
                    return Err(KError::invalid_input(format!("板块类别 {kind:?} 不在域里：{}", pack.section_kinds.join("、"))));
                }
                let paths = check_paths(&ctx, paths)?;
                if questions.is_empty() {
                    return Err(KError::invalid_input("板块至少要有一个问题"));
                }
                work.next_section += 1;
                let id = format!("s-{:02}", work.next_section);
                work.sections.push(Section {
                    id: id.clone(),
                    label: label.clone(),
                    kind: kind.clone(),
                    paths,
                    questions: questions.clone(),
                    disposition: None,
                    disposition_reason: None,
                    reopened: false,
                    reopened_at: None,
                    merged_into: None,
                    merged_at: None,
                    abolished: None,
                    warnings: vec![],
                    created_at: now.to_string(),
                });
                ids.push(id);
            }
            Op::SectionUpdate { id, label, paths, questions } => {
                let i = live_section(&work, id)?;
                if let Some(p) = paths {
                    let p = check_paths(&ctx, &p)?;
                    if p.is_empty() {
                        return Err(KError::invalid_input("板块范围不能为空"));
                    }
                    work.sections[i].paths = p;
                }
                if let Some(q) = questions {
                    if q.is_empty() {
                        return Err(KError::invalid_input("板块问题不能为空"));
                    }
                    work.sections[i].questions = q.clone();
                }
                if let Some(l) = label {
                    work.sections[i].label = l.clone();
                }
                ids.push(id.clone());
            }
            Op::SectionMerge { id, into } => {
                let i = live_section(&work, id)?;
                let j = live_section(&work, into)?;
                if i == j {
                    return Err(KError::new("E_MERGE_CYCLE", "板块不能并入自己"));
                }
                let src = work.sections[i].clone();
                for p in src.paths {
                    if !work.sections[j].paths.contains(&p) {
                        work.sections[j].paths.push(p);
                    }
                }
                for q in src.questions {
                    if !work.sections[j].questions.contains(&q) {
                        work.sections[j].questions.push(q);
                    }
                }
                for f in work.findings.iter_mut() {
                    if f.section_id == src.id {
                        f.section_id = into.clone();
                    }
                }
                work.sections[i].merged_into = Some(into.clone());
                work.sections[i].merged_at = Some(now.to_string());
                work.sections[i].reopened = false;
                ids.push(id.clone());
                ids.push(into.clone());
            }
            Op::SectionAbolish { id, reason } => {
                let i = live_section(&work, id)?;
                let n = effective_in(&work, id);
                if n > 0 {
                    return Err(KError::new("E_SECTION_HAS_FINDINGS", text::e_section_has_findings(&work.sections[i].label, n)).with_data(json!({ "count": n })));
                }
                work.sections[i].abolished = Some(AbolishInfo { reason: reason.clone(), at: now.to_string() });
                work.sections[i].reopened = false;
                ids.push(id.clone());
            }
            Op::FindingCreate { section_id, kind, identity, anchors, fields } => {
                live_section(&work, section_id)?;
                if !pack.finding_kinds.contains(kind) {
                    return Err(KError::invalid_input(format!("断言类别 {kind:?} 不在域里：{}", pack.finding_kinds.join("、"))));
                }
                identity::check(&identity)?;
                for a in anchors {
                    check_anchor(&ctx, a)?;
                }
                let fp = crate::identity::fingerprint(&scan.target_key, &scan.domain, kind, &identity.anchor, identity.instance.as_deref());
                if let Some(other) = work.findings.iter().find(|f| f.fingerprint == fp && f.is_effective()) {
                    return Err(KError::new("E_IDENTITY_CONFLICT", text::e_identity_conflict(kind, &identity.anchor, &other.id)));
                }
                work.next_finding += 1;
                let id = format!("f-{:03}", work.next_finding);
                let mut f = Finding {
                    id: id.clone(),
                    section_id: section_id.clone(),
                    kind: kind.clone(),
                    identity: identity.clone(),
                    fingerprint: fp,
                    anchors: anchors.clone(),
                    anchor_refs: vec![],
                    status: FindingStatus::Candidate,
                    fields: fields.clone(),
                    paths: vec![],
                    retracted_paths: vec![],
                    warnings: vec![],
                    retracted: None,
                    created_at: now.to_string(),
                    updated_at: now.to_string(),
                };
                f.anchor_refs = anchors.iter().map(|a| check_anchor(&ctx, a).map(|r| r.refs)).collect::<KResult<Vec<_>>>()?.into_iter().flatten().collect();
                work.findings.push(f);
                ids.push(id);
            }
            Op::FindingUpdate { id, anchors, fields } => {
                let i = live_finding(&work, id)?;
                if let Some(a) = anchors {
                    for s in a {
                        check_anchor(&ctx, s)?;
                    }
                    work.findings[i].anchors = a.clone();
                    work.findings[i].anchor_refs = a.iter().map(|s| check_anchor(&ctx, s).map(|r| r.refs)).collect::<KResult<Vec<_>>>()?.into_iter().flatten().collect();
                }
                for (k, v) in fields {
                    work.findings[i].fields.insert(k.clone(), v.clone());
                }
                work.findings[i].updated_at = now.to_string();
                ids.push(id.clone());
            }
            Op::FindingRetract { id, reason } => {
                let i = live_finding(&work, id)?;
                work.findings[i].retracted = Some(RetractInfo { reason: reason.clone(), at: now.to_string() });
                ids.push(id.clone());
            }
            Op::DispositionSet { section_id, disposition, reason } => {
                let i = live_section(&work, section_id)?;
                if !pack.dispositions.contains(disposition) {
                    return Err(KError::invalid_input(format!("处置 {disposition:?} 不在域里：{}", pack.dispositions.join("、"))));
                }
                let n = effective_in(&work, section_id);
                if n > 0 {
                    return Err(KError::new("E_SECTION_HAS_FINDINGS", format!("板块 {} 有 {n} 条有效断言，不需要处置；处置是给没有有效断言的板块的", work.sections[i].label)));
                }
                work.sections[i].disposition = Some(disposition.clone());
                work.sections[i].disposition_reason = Some(reason.clone());
                ids.push(section_id.clone());
            }
            Op::VerdictAttach { id, fields } => {
                let i = live_finding(&work, id)?;
                for (k, v) in fields {
                    work.findings[i].fields.insert(k.clone(), v.clone());
                }
                work.findings[i].updated_at = now.to_string();
                ids.push(id.clone());
            }
            Op::EvidenceAppend { id, anchors } => {
                let i = live_finding(&work, id)?;
                for a in anchors {
                    check_anchor(&ctx, a)?;
                }
                let entry = work.findings[i].fields.entry("evidence".to_string()).or_insert_with(|| Value::Array(vec![]));
                if !entry.is_array() {
                    *entry = Value::Array(vec![]);
                }
                if let Value::Array(arr) = entry {
                    for a in anchors {
                        arr.push(Value::String(a.clone()));
                    }
                }
                work.findings[i].updated_at = now.to_string();
                ids.push(id.clone());
            }
            Op::PathAttach { id, steps, prerequisites } => {
                let i = live_finding(&work, id)?;
                check_steps(&ctx, steps)?;
                work.findings[i].paths.push(PathRecord { steps: steps.clone(), prerequisites: prerequisites.clone(), warnings: vec![], attached_at: now.to_string() });
                ids.push(id.clone());
            }
            Op::PathUpdate { id, index, steps, prerequisites } => {
                let i = live_finding(&work, id)?;
                if *index >= work.findings[i].paths.len() {
                    return Err(KError::new("E_UNKNOWN_RECORD", format!("断言 {id} 没有第 {index} 条路径")));
                }
                check_steps(&ctx, steps)?;
                work.findings[i].paths[*index].steps = steps.clone();
                work.findings[i].paths[*index].prerequisites = prerequisites.clone();
                ids.push(id.clone());
            }
            Op::PathRetract { id, index, reason } => {
                let i = live_finding(&work, id)?;
                if *index >= work.findings[i].paths.len() {
                    return Err(KError::new("E_UNKNOWN_RECORD", format!("断言 {id} 没有第 {index} 条路径")));
                }
                let p = work.findings[i].paths.remove(*index);
                work.findings[i].retracted_paths.push(RetractedPath { path: p, reason: reason.clone(), at: now.to_string() });
                ids.push(id.clone());
            }
            Op::ExclusionAdd { path, reason } => {
                let n = norm_rel(path);
                if !inventory.contains_path(&n) {
                    return Err(KError::new("E_EXCLUSION_UNKNOWN", format!("排除 {path:?} 不在清点范围内")));
                }
                if work.exclusions.iter().any(|e| e.path == n) {
                    return Err(KError::invalid_input(format!("{n:?} 已经排除了")));
                }
                work.exclusions.push(Exclusion { path: n, reason: reason.clone(), added_at: Some(now.to_string()) });
                ids.push(format!("exclude:{path}"));
            }
            Op::ExclusionRemove { path } => {
                let n = norm_rel(path);
                let before = work.exclusions.len();
                work.exclusions.retain(|e| e.path != n);
                if work.exclusions.len() == before {
                    return Err(KError::new("E_UNKNOWN_RECORD", format!("排除 {path:?} 不存在")));
                }
                ids.push(format!("include:{path}"));
            }
        }
    }

    commit(&mut work, &ids);
    gates::refresh_touched(scan, &mut work, templates, scan.phase, &ids);

    // 写检查点、状态、head。
    let warnings = warnings_for(&work, &ids);
    let outcome_pending = Outcome {
        written: true,
        duplicate: false,
        ids: ids.clone(),
        warnings: warnings.clone(),
        phase: scan.phase,
        advanced_from: None,
        gaps: vec![],
        checkpoint: Some(input_digest.clone()),
    };
    write_checkpoint(store, scan, ops, &input_digest, &outcome_pending, now, session_id)?;

    // 评估下一关：过了就自动推进；finalized 只能声明，但缺口照样算给 agent 看。
    let mut advanced_from = None;
    let mut gaps = Vec::new();
    if let Some(next) = scan.phase.next() {
        let mut probe = work.clone();
        gates::refresh_all(scan, &mut probe, templates, next);
        let env = GateEnv { scan, inventory, pack, templates, policy_text: None };
        gaps = gates::evaluate(next, &probe, &env);
        if gaps.is_empty() && next != Phase::Finalized {
            work = probe;
            advanced_from = Some(scan.phase);
            scan.phase = next;
            if next == Phase::Validation {
                scan.templates.frozen = true;
            }
            scan.updated_at = now.to_string();
        }
    }

    let outcome = Outcome {
        written: true,
        duplicate: false,
        ids: {
            let mut seen = BTreeSet::new();
            ids.into_iter().filter(|i| seen.insert(i.clone())).collect()
        },
        warnings,
        phase: scan.phase,
        advanced_from,
        gaps: gaps.clone(),
        checkpoint: Some(input_digest.clone()),
    };
    // 检查点里存最终结果，重试时原样返回。
    write_checkpoint(store, scan, ops, &input_digest, &outcome, now, session_id)?;
    store.write_scan(scan)?;
    store.write_state(&work)?;
    *state = work;
    Ok(outcome)
}

fn warnings_for(state: &State, ids: &[String]) -> Vec<RecordWarnings> {
    let mut out = Vec::new();
    for f in &state.findings {
        if ids.contains(&f.id) && !f.warnings.is_empty() {
            out.push(RecordWarnings { record_id: f.id.clone(), warnings: f.warnings.clone() });
        }
    }
    for s in &state.sections {
        if ids.contains(&s.id) && !s.warnings.is_empty() {
            out.push(RecordWarnings { record_id: s.id.clone(), warnings: s.warnings.clone() });
        }
    }
    out
}

fn head_path(store: &ScanStore) -> std::path::PathBuf {
    store.checkpoint_head()
}

fn read_head(store: &ScanStore) -> KResult<(String, std::path::PathBuf)> {
    let v: Value = crate::store::ScanStore::read(&head_path(store)).map_err(crate::store::io_err)?;
    let digest = v["inputDigest"].as_str().unwrap_or("").to_string();
    let cp = v["checkpoint"].as_str().unwrap_or("");
    Ok((digest, store.checkpoints_dir().join(format!("{cp}.json"))))
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Checkpoint {
    digest: String,
    at: String,
    writer: String,
    input: Vec<Value>,
    result: Outcome,
}

fn write_checkpoint(
    store: &ScanStore,
    scan: &Scan,
    ops: &[Value],
    digest: &str,
    outcome: &Outcome,
    now: &str,
    session_id: &str,
) -> KResult<()> {
    let _ = scan;
    let cp = Checkpoint { digest: digest.to_string(), at: now.to_string(), writer: session_id.to_string(), input: ops.to_vec(), result: outcome.clone() };
    let path = store.checkpoints_dir().join(format!("{digest}.json"));
    crate::store::ScanStore::write_json(&path, &cp).map_err(crate::store::io_err)?;
    let head = json!({ "checkpoint": digest, "inputDigest": digest, "at": now });
    crate::store::ScanStore::write_json(&head_path(store), &head).map_err(crate::store::io_err)
}

mod identity {
    use crate::error::{KError, KResult};
    use crate::model::Identity;

    pub fn check(id: &Identity) -> KResult<()> {
        crate::identity::validate_anchor_slug(&id.anchor).map_err(KError::invalid_input)?;
        if let Some(inst) = &id.instance {
            crate::identity::validate_instance(inst).map_err(KError::invalid_input)?;
        }
        Ok(())
    }
}
