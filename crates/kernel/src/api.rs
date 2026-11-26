//! 对外函数：MCP 服务和命令行都只调这里。返回统一的 JSON 形状；每次工具调用写一条日志。

use crate::error::{KError, KResult};
use crate::gates::{self, GateEnv};
use crate::inventory::{self, Inventory};
use crate::log::{self, LogEntry, LogFilter};
use crate::model::*;
use crate::pack::{self, Pack, METHOD_STAGES};
use crate::progress;
use crate::record;
use crate::status;
use crate::store::{Home, ScanStore};
use crate::templates::{self, TemplateSet};
use crate::text;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub pid: u32,
    pub since: String,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Session { id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(), pid: std::process::id(), since: crate::now() }
    }
}

pub struct Loaded {
    pub store: ScanStore,
    pub scan: Scan,
    pub state: State,
    pub progress: Progress,
    pub templates: TemplateSet,
    pub inventory: Inventory,
    pub pack: Pack,
}

fn with_lock<T>(store: &ScanStore, exclusive: bool, f: impl FnOnce() -> KResult<T>) -> KResult<T> {
    store.locked(exclusive, f).map_err(crate::store::io_err)?
}

fn load(store: &ScanStore) -> KResult<Loaded> {
    let scan = store.read_scan()?;
    let state = store.read_state()?;
    let progress = store.read_progress()?;
    let templates = TemplateSet::read_dir(&store.templates_dir())?;
    let inventory = store.read_inventory()?;
    let pack = pack::load_snapshot(&store.pack_dir(), &store.home, &scan.root, &scan.domain)?;
    Ok(Loaded { store: store.clone(), scan, state, progress, templates, inventory, pack })
}

fn env<'a>(l: &'a Loaded, policy_text: Option<&'a str>) -> GateEnv<'a> {
    GateEnv { scan: &l.scan, inventory: &l.inventory, pack: &l.pack, templates: &l.templates, policy_text }
}

fn policy_text(store: &ScanStore) -> Option<String> {
    std::fs::read_to_string(store.policy_path()).ok()
}

fn table_stage(phase: Phase) -> Phase {
    match phase {
        Phase::Preflight | Phase::Planning => Phase::Planning,
        Phase::Discovery => Phase::Discovery,
        Phase::Validation => Phase::Validation,
        Phase::AnalyzeFullPath | Phase::Finalized => Phase::AnalyzeFullPath,
    }
}

// ─── 日志 ────────────────────────────────────────────────────────────────────

fn classify(result: &KResult<Value>) -> (&'static str, Option<String>, Vec<Gap>, Option<String>, Option<String>) {
    match result {
        Ok(v) => {
            let gaps: Vec<Gap> = serde_json::from_value(v["gaps"].clone()).unwrap_or_default();
            let has_warnings = v["warnings"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
            let result = if has_warnings {
                "warnings"
            } else if !gaps.is_empty() {
                "rejected"
            } else {
                "ok"
            };
            (result, v["message"].as_str().map(|s| s.to_string()), gaps, v["guidance"].as_str().map(|s| s.to_string()), v["checkpoint"].as_str().map(|s| s.to_string()))
        }
        Err(e) => ("error", Some(format!("{e}")), vec![], None, None),
    }
}

fn log_call(store: Option<&ScanStore>, session: Option<&Session>, tool: &str, input: &Value, result: &KResult<Value>, started: Instant) {
    let Some(store) = store else {
        return;
    };
    let (result_kind, message, gaps, guidance, checkpoint) = classify(result);
    let _ = log::append(
        store,
        LogEntry {
            seq: 0,
            at: crate::now(),
            kind: "call".into(),
            writer: session.map(|s| s.id.clone()),
            tool: Some(tool.to_string()),
            input: Some(input.clone()),
            result: Some(result_kind.to_string()),
            message,
            gaps,
            guidance,
            checkpoint,
            ms: Some(started.elapsed().as_millis() as u64),
            ..Default::default()
        },
    );
}

fn log_event(store: &ScanStore, session: Option<&Session>, event: &str, source: &str, message: String, detail: Value, gaps: Vec<Gap>) {
    let _ = log::append(
        store,
        LogEntry {
            seq: 0,
            at: crate::now(),
            kind: "event".into(),
            writer: session.map(|s| s.id.clone()),
            event: Some(event.to_string()),
            source: Some(source.to_string()),
            message: Some(message),
            gaps,
            detail,
            ..Default::default()
        },
    );
}

// ─── 概要视图 ────────────────────────────────────────────────────────────────

pub fn summary_view(l: &Loaded) -> KResult<Value> {
    let scan = &l.scan;
    let state = &l.state;
    let sealed = scan.sealed_at.is_some();
    let (next_gate, gaps) = if sealed {
        (None, vec![])
    } else {
        let (t, g) = gates::preview_next(scan, state, &env(l, None));
        (Some(t), g)
    };
    let uncovered = inventory::uncovered(&l.inventory, state.active_sections(), &state.exclusions);
    let ex: Vec<String> = state.exclusions.iter().map(|e| e.path.clone()).collect();
    let excluded = l.inventory.files.iter().filter(|f| inventory::excluded_by(&f.path, &ex)).count();
    let total = l.inventory.files.len();
    let sections: Vec<Value> = state
        .sections
        .iter()
        .map(|s| {
            let mut counts: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
            for f in state.findings.iter().filter(|f| f.section_id == s.id && f.is_effective()) {
                *counts.entry(f.status.as_str().to_string()).or_insert(0) += 1;
            }
            json!({
                "id": s.id, "label": s.label, "kind": s.kind,
                "status": status::section_status(s, state).as_str(),
                "reopened": s.reopened,
                "paths": s.paths, "questions": s.questions,
                "disposition": s.disposition, "dispositionReason": s.disposition_reason,
                "findingCounts": counts, "warnings": s.warnings.len(),
                "mergedInto": s.merged_into, "abolished": s.abolished,
            })
        })
        .collect();
    let guidance = text::guidance(scan.phase).to_string();
    let next_step = text::next_step(scan.phase, &gaps, progress_waiting(scan, l));
    let st = table_stage(scan.phase);
    let c = status::counts(state, &l.pack);
    Ok(json!({
        "scan": {
            "scanId": scan.scan_id, "domain": scan.domain, "mode": scan.mode, "phase": scan.phase,
            "root": scan.root, "remote": scan.remote, "revision": scan.revision, "dirty": scan.dirty,
            "snapshotDigest": scan.snapshot_digest, "sealed": sealed, "sealedAt": scan.sealed_at,
            "writer": scan.writer, "createdAt": scan.created_at, "updatedAt": scan.updated_at,
        },
        "templates": { "digest": l.templates.digest, "frozen": scan.templates.frozen },
        "pack": {
            "domain": l.scan.domain, "description": l.pack.description,
            "source": l.scan.pack.source, "digest": l.scan.pack.digest, "overlays": l.scan.pack.overlays,
            "sectionKinds": l.pack.section_kinds, "findingKinds": l.pack.finding_kinds,
            "pathRequiredKinds": l.pack.path_required_kinds, "pathRoles": l.pack.path_roles,
            "originRoles": l.pack.origin_roles, "outcomeRoles": l.pack.outcome_roles,
            "dispositions": l.pack.dispositions, "anchorNaming": l.pack.anchor_naming,
            "policyFile": l.pack.policy_file,
        },
        // 当前阶段的方法正文：agent 每次读上下文都带上，不用记流程。preflight / finalized 为 null。
        "method": method_json(&l.pack, l.scan.phase),
        "userRequests": scan.user_requests,
        "progress": progress_json(l),
        "counts": {
            "sections": c.sections,
            "findings": c.findings,
            "withWarnings": c.with_warnings,
            "paths": { "required": c.paths_required, "complete": c.paths_complete, "incomplete": c.paths_incomplete },
            "inventory": { "files": total, "covered": total - uncovered.len() - excluded, "excluded": excluded, "uncovered": uncovered.len() },
        },
        "sections": sections,
        "gaps": gaps,
        "nextGate": next_gate,
        "guidance": guidance,
        "nextStep": next_step,
        "fragments": l.templates.field_tables(st),
        "policy": policy_text(&l.store),
    }))
}

/// 当前阶段的方法：来自哪个文件、正文。
fn method_json(pack: &Pack, phase: Phase) -> Value {
    match phase.as_str() {
        "planning" | "discovery" | "validation" | "analyze_full_path" => json!({
            "stage": phase.as_str(),
            "path": pack.method.get(phase.as_str()),
            "content": pack.method_for(phase),
        }),
        _ => Value::Null,
    }
}

fn progress_waiting<'a>(_scan: &Scan, l: &'a Loaded) -> Option<&'a str> {
    l.progress.waiting_for_decision.as_deref()
}

fn progress_json(l: &Loaded) -> Value {
    let reopened: Vec<String> = l.state.sections.iter().filter(|s| s.reopened && s.is_live()).map(|s| s.id.clone()).collect();
    json!({
        "activeSections": l.progress.active_sections,
        "note": l.progress.note,
        "waitingForDecision": l.progress.waiting_for_decision,
        "reopenedSections": reopened,
        "updatedAt": l.progress.updated_at,
    })
}

fn response(scan: &Scan, extra: Map<String, Value>) -> Value {
    let mut m = Map::new();
    m.insert("ok".into(), Value::Bool(true));
    m.insert("scanId".into(), json!(scan.scan_id));
    m.insert("phase".into(), json!(scan.phase));
    for (k, v) in extra {
        m.insert(k, v);
    }
    Value::Object(m)
}

// ─── scan_start ──────────────────────────────────────────────────────────────

/// 策略文件里追加的排除：形如 `exclude: path` 或 `- exclude: path` 的行。
pub fn parse_policy_exclusions(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r"(?i)^\s*(?:-\s*)?exclude:\s*(\S+)\s*$").unwrap();
    text.lines().filter_map(|l| re.captures(l).map(|c| c[1].to_string())).collect()
}

pub fn start(home: &Home, session: &Session, input: &Value) -> KResult<Value> {
    let started = Instant::now();
    let Value::Object(m) = input else {
        return Err(KError::invalid_input("输入应是对象"));
    };
    let result = match m.get("resume") {
        Some(Value::String(id)) if !id.trim().is_empty() => resume(home, session, input, id.trim()),
        _ => start_new(home, session, &m),
    };
    if let Ok(v) = &result {
        if let Some(id) = v["scan"]["scanId"].as_str() {
            if let Ok(store) = ScanStore::open(home.clone(), id) {
                log_call(Some(&store), Some(session), "scan_start", input, &result, started);
            }
        }
    }
    result
}

fn resume(home: &Home, session: &Session, input: &Value, id: &str) -> KResult<Value> {
    let store = ScanStore::open(home.clone(), id)?;
    let sealed = store.is_sealed();
    let session2 = session.clone();
    with_lock(&store, !sealed, || {
        let mut l = load(&store)?;
        if !sealed {
            let old = l.scan.writer.session_id.clone();
            l.scan.writer = Writer { session_id: session2.id.clone(), pid: session2.pid, since: session2.since.clone() };
            l.scan.updated_at = crate::now();
            l.store.write_scan(&l.scan)?;
            if old != session2.id {
                log_event(&l.store, Some(&session2), "takeover", "mcp", format!("写者从 {old} 换成 {}", session2.id), json!({ "from": old, "to": session2.id }), vec![]);
            }
        }
        let mut v = summary_view(&l)?;
        v["resumed"] = Value::Bool(true);
        v["sealed"] = Value::Bool(sealed);
        if sealed {
            v["message"] = json!("扫描已封存，只读；要改就从它起一次增量扫描");
        } else {
            v["message"] = json!("已接管，继续；先看 gaps 和 nextStep");
        }
        let _ = input;
        Ok(v)
    })
}

fn start_new(home: &Home, session: &Session, m: &Map<String, Value>) -> KResult<Value> {
    let domain = match m.get("domain") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return Err(KError::invalid_input(format!("domain 必填：{}", pack::domain_names(home, &std::path::Path::new(".")).join("、")))),
    };
    if m.get("incrementalFrom").map_or(false, |v| !v.is_null()) {
        return Err(KError::new("E_NOT_IMPLEMENTED", "增量模式这一版还没做；先全量"));
    }
    let prompt = match m.get("prompt") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        _ => return Err(KError::invalid_input("prompt 必填：用户的要求，原话")),
    };
    let root: PathBuf = match m.get("root") {
        Some(Value::String(s)) if !s.trim().is_empty() => PathBuf::from(shellexpand(s.trim())),
        _ => std::env::current_dir().map_err(crate::store::io_err)?,
    };
    let root = root.canonicalize().map_err(|e| KError::new("E_TARGET_INVALID", format!("{} 不是可访问的目录：{e}", root.display())))?;
    if !root.is_dir() {
        return Err(KError::new("E_TARGET_INVALID", format!("{} 不是目录", root.display())));
    }
    // 域包按三层（内嵌 / 用户级 / 项目级）合并后在这一刻固定：规则、模板、方法都取现在的。
    let resolved = pack::resolve(home, &root, &domain)?;
    let pack = resolved.pack.clone();
    let (policy_name, policy_text) = inventory::read_policy(&root, &pack).map(|(n, t)| (Some(n), Some(t))).unwrap_or((None, None));
    let policy_exclusions = policy_text.as_deref().map(parse_policy_exclusions).unwrap_or_default();
    let pre = inventory::run(&root, &pack, &policy_exclusions)?;
    let id = crate::store::new_scan_id();
    let key = crate::store::target_key(pre.remote.as_deref(), &root);
    let store = ScanStore::create(home.clone(), &key, &id).map_err(crate::store::io_err)?;
    let templates = TemplateSet::build(resolved.template_files())?;
    templates.write_dir(&store.templates_dir())?;
    pack::write_snapshot(&store.pack_dir(), &resolved)?;
    let now = crate::now();
    let scan = Scan {
        schema: SCHEMA,
        scan_id: id.clone(),
        target_key: key,
        root: root.clone(),
        remote: pre.remote.clone(),
        revision: pre.revision.clone(),
        dirty: pre.dirty,
        snapshot_digest: pre.snapshot_digest.clone(),
        domain,
        mode: "full".into(),
        base_scan_id: None,
        phase: Phase::Preflight,
        writer: Writer { session_id: session.id.clone(), pid: session.pid, since: session.since.clone() },
        user_requests: vec![UserRequest { text: prompt, at: now.clone(), source: "prompt".into() }],
        policy: Policy {
            files: policy_name.iter().cloned().collect(),
            digest: policy_text.as_deref().map(|t| crate::sha256_hex(t.as_bytes())),
        },
        templates: TemplatesMeta { digest: templates.digest.clone(), frozen: false },
        pack: resolved.meta.clone(),
        sealed_at: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let mut inv = pre.inventory.clone();
    inv.revision = pre.revision.clone();
    store.write_scan(&scan)?;
    store.write_state(&State { schema: SCHEMA, ..Default::default() })?;
    store.write_progress(&Progress::default())?;
    ScanStore::write_json(&store.inventory_json(), &inv).map_err(crate::store::io_err)?;
    if let Some(t) = &policy_text {
        std::fs::write(store.policy_path(), t).map_err(crate::store::io_err)?;
    }
    let l = load(&store)?;
    let mut v = summary_view(&l)?;
    v["message"] = json!("已创建。首次读上下文后进入 planning；先看 pack 和 inventory，再画项目地图");
    Ok(v)
}

fn shellexpand(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest).to_string_lossy().into_owned();
        }
    }
    s.to_string()
}

// ─── scan_context ────────────────────────────────────────────────────────────

pub fn context(home: &Home, session: &Session, input: &Value) -> KResult<Value> {
    let started = Instant::now();
    let result = context_inner(home, session, input);
    if let Some(id) = input["scanId"].as_str() {
        if let Ok(store) = ScanStore::open(home.clone(), id) {
            log_call(Some(&store), Some(session), "scan_context", input, &result, started);
        }
    }
    result
}

fn context_inner(home: &Home, session: &Session, input: &Value) -> KResult<Value> {
    let id = input["scanId"].as_str().ok_or_else(|| KError::invalid_input("scanId 必填"))?;
    let store = ScanStore::open(home.clone(), id)?;
    let view = input["view"].as_str().unwrap_or("summary");
    let phase0 = store.read_scan().map(|s| s.phase).unwrap_or(Phase::Preflight);
    // 首次由写者读取且阶段为 preflight → 进入 planning
    let exclusive = view == "summary" && phase0 == Phase::Preflight;
    let session = session.clone();
    with_lock(&store, exclusive, || {
        let mut l = load(&store)?;
        if exclusive && l.scan.writer.session_id == session.id && l.scan.phase == Phase::Preflight {
            l.scan.phase = Phase::Planning;
            l.scan.updated_at = crate::now();
            l.store.write_scan(&l.scan)?;
            log_event(&l.store, Some(&session), "gate", "kernel", "首次读取上下文，进入 planning".into(), json!({ "from": "preflight", "to": "planning", "declared": false }), vec![]);
        }
        match view {
            "summary" => summary_view(&l),
            "fragments" => Ok(json!({
                "scanId": l.scan.scan_id, "phase": l.scan.phase,
                "tables": {
                    "section": { "planning": l.templates.field_table("section", Phase::Planning) },
                    "exclusion": { "planning": l.templates.field_table("exclusion", Phase::Planning) },
                    "finding": {
                        "discovery": l.templates.field_table("finding", Phase::Discovery),
                        "validation": l.templates.field_table("finding", Phase::Validation),
                        "analyze_full_path": l.templates.field_table("finding", Phase::AnalyzeFullPath),
                    },
                    "path": { "analyze_full_path": l.templates.field_table("path", Phase::AnalyzeFullPath) },
                },
                "anchorsFormat": text::ANCHOR_FORMAT,
                "pathRoles": l.pack.path_roles, "originRoles": l.pack.origin_roles, "outcomeRoles": l.pack.outcome_roles,
            })),
            "templates" => {
                let files: Vec<Value> = l.templates.files.values().map(|f| json!({ "path": f.path, "meta": f.meta.to_value(), "content": f.body })).collect();
                let partials: Vec<Value> = l.templates.partials.iter().map(|(p, c)| json!({ "path": p, "content": c })).collect();
                Ok(json!({ "scanId": l.scan.scan_id, "digest": l.templates.digest, "frozen": l.scan.templates.frozen, "files": files, "partials": partials }))
            }
            "records" => records_view(&l, input),
            "inventory" => inventory_view(&l, input),
            "pack" => Ok(json!({
                "scanId": l.scan.scan_id, "domain": l.scan.domain, "description": l.pack.description,
                "source": l.scan.pack.source, "digest": l.scan.pack.digest, "overlays": l.scan.pack.overlays,
                "rules": l.pack.to_value(),
                "method": METHOD_STAGES.iter().filter_map(|s| l.pack.method.get(*s).map(|p| json!({ "stage": s, "path": p, "content": l.pack.docs.get(p) }))).collect::<Vec<_>>(),
                "references": l.pack.references.iter().map(|p| json!({ "path": p, "content": l.pack.docs.get(p) })).collect::<Vec<_>>(),
            })),
            other => Err(KError::invalid_input(format!("view {other:?} 不认识：summary | fragments | templates | pack | records | inventory"))),
        }
    })
}

fn records_view(l: &Loaded, input: &Value) -> KResult<Value> {
    let limit = input["limit"].as_u64().unwrap_or(50).clamp(1, 200) as usize;
    let cursor = input["cursor"].as_u64().unwrap_or(0) as usize;
    let mut items: Vec<Value> = Vec::new();
    if let Some(Value::Array(ids)) = input.get("ids") {
        for id in ids.iter().filter_map(|v| v.as_str()) {
            if let Some(f) = l.state.finding(id) {
                items.push(json!(f));
            } else if let Some(s) = l.state.section(id) {
                items.push(json!({ "section": s, "findings": l.state.findings.iter().filter(|f| f.section_id == s.id).collect::<Vec<_>>() }));
            } else {
                items.push(json!({ "missing": id }));
            }
        }
        return Ok(json!({ "scanId": l.scan.scan_id, "items": items, "total": items.len() }));
    }
    let r#type = input["type"].as_str().unwrap_or("finding");
    let all: Vec<Value> = if r#type == "section" {
        l.state.sections.iter().map(|s| json!(s)).collect()
    } else {
        l.state
            .findings
            .iter()
            .filter(|f| input["sectionId"].as_str().map_or(true, |x| f.section_id == x))
            .filter(|f| input["status"].as_str().map_or(true, |x| f.status.as_str() == x))
            .filter(|f| input["kind"].as_str().map_or(true, |x| f.kind == x))
            .map(|f| json!(f))
            .collect()
    };
    let total = all.len();
    let next = (cursor + limit).min(total);
    items.extend_from_slice(&all[cursor.min(total)..next]);
    Ok(json!({ "scanId": l.scan.scan_id, "items": items, "total": total, "nextCursor": if next < total { Some(next) } else { None } }))
}

fn inventory_view(l: &Loaded, input: &Value) -> KResult<Value> {
    let unc = inventory::uncovered(&l.inventory, l.state.active_sections(), &l.state.exclusions);
    let ex: Vec<String> = l.state.exclusions.iter().map(|e| e.path.clone()).collect();
    let mark = |p: &str| {
        if inventory::excluded_by(p, &ex) {
            "excluded"
        } else if unc.iter().any(|u| u == p) {
            "uncovered"
        } else {
            "covered"
        }
    };
    let cover_of = |p: &str| -> Vec<String> {
        l.state.active_sections().filter(|s| inventory::is_covered(p, &s.paths)).map(|s| s.id.clone()).collect()
    };
    let mut dirs: std::collections::BTreeMap<String, (usize, usize, usize, usize)> = std::collections::BTreeMap::new();
    for f in &l.inventory.files {
        let top = f.path.split('/').next().unwrap_or(&f.path).to_string();
        let e = dirs.entry(top).or_default();
        e.0 += 1;
        match mark(&f.path) {
            "covered" => e.1 += 1,
            "excluded" => e.2 += 1,
            _ => e.3 += 1,
        }
    }
    let directories: Vec<Value> = dirs.into_iter().map(|(p, (n, c, e, u))| json!({ "path": p, "files": n, "covered": c, "excluded": e, "uncovered": u })).collect();
    let filter = input["mark"].as_str();
    let prefix = input["prefix"].as_str();
    let files: Vec<&inventory::InvFile> = l
        .inventory
        .files
        .iter()
        .filter(|f| prefix.map_or(true, |p| f.path.starts_with(p)))
        .filter(|f| filter.map_or(true, |m| mark(&f.path) == m))
        .collect();
    let limit = input["limit"].as_u64().unwrap_or(100).clamp(1, 500) as usize;
    let cursor = input["cursor"].as_u64().unwrap_or(0) as usize;
    let total = files.len();
    let next = (cursor + limit).min(total);
    let page: Vec<Value> = files[cursor.min(total)..next].iter().map(|f| json!({ "path": f.path, "size": f.size, "mark": mark(&f.path), "sections": cover_of(&f.path) })).collect();
    Ok(json!({
        "scanId": l.scan.scan_id,
        "total": l.inventory.files.len(),
        "uncovered": unc.len(),
        "excluded": l.inventory.files.iter().filter(|f| inventory::excluded_by(&f.path, &ex)).count(),
        "defaultExclusions": l.inventory.default_exclusions,
        "policyExclusions": l.inventory.policy_exclusions,
        "directories": directories,
        "files": page,
        "nextCursor": if next < total { Some(next) } else { None },
    }))
}

// ─── scan_templates ──────────────────────────────────────────────────────────

pub fn templates_write(home: &Home, session: &Session, input: &Value) -> KResult<Value> {
    let started = Instant::now();
    let result = templates_write_inner(home, session, input);
    if let Some(id) = input["scanId"].as_str() {
        if let Ok(store) = ScanStore::open(home.clone(), id) {
            log_call(Some(&store), Some(session), "scan_templates", input, &result, started);
        }
    }
    result
}

fn templates_write_inner(home: &Home, session: &Session, input: &Value) -> KResult<Value> {
    let id = input["scanId"].as_str().ok_or_else(|| KError::invalid_input("scanId 必填"))?;
    let store = ScanStore::open(home.clone(), id)?;
    let mut changes: Vec<(String, String)> = Vec::new();
    for key in ["add", "tighten"] {
        if let Some(Value::Array(a)) = input.get(key) {
            for (i, v) in a.iter().enumerate() {
                let path = v["path"].as_str().ok_or_else(|| KError::invalid_input(format!("{key}[{i}] 缺少 path")))?;
                let content = v["content"].as_str().ok_or_else(|| KError::invalid_input(format!("{key}[{i}] 缺少 content")))?;
                changes.push((path.to_string(), content.to_string()));
            }
        }
    }
    if changes.is_empty() {
        return Err(KError::invalid_input("add 或 tighten 至少给一个 { path, content }"));
    }
    with_lock(&store, true, || {
        let mut l = load(&store)?;
        crate::store::writer_check(&l.scan, &session.id)?;
        if l.scan.sealed_at.is_some() || l.store.is_sealed() {
            return Err(KError::new("E_SEALED", "扫描已封存，模板集不可改"));
        }
        let mut current: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
        for (p, f) in &l.templates.files {
            current.insert(p.clone(), f.text.clone());
        }
        current.extend(l.templates.partials.clone());
        let mut touched: Vec<String> = Vec::new();
        for (path, content) in &changes {
            let path = path.trim().trim_start_matches("./").trim_start_matches('/').to_string();
            if !(path.starts_with("fragments/") || path.starts_with("outputs/") || path.starts_with("partials/")) {
                return Err(KError::new("E_TEMPLATE_INVALID", format!("{path}：要放在 fragments/、outputs/ 或 partials/ 下")));
            }
            if !path.starts_with("partials/") {
                let new = templates::parse_file(&path, content)?;
                if let Some(old_content) = current.get(&path) {
                    let old = templates::parse_file(&path, old_content)?;
                    match (&old.meta, &new.meta) {
                        (templates::Meta::Fragment { .. }, templates::Meta::Output { .. }) | (templates::Meta::Output { .. }, templates::Meta::Fragment { .. }) => {
                            return Err(KError::new("E_TEMPLATE_INVALID", format!("{path}：不能在片段和输出之间改类型")));
                        }
                        _ => {}
                    }
                    if let Some(why) = templates::loosen(&old, &new) {
                        return Err(KError::new("E_TEMPLATE_LOOSENED", format!("模板只能追加和收紧，不能放宽或删除：{path} {why}")));
                    }
                    if l.scan.templates.frozen && matches!(old.meta, templates::Meta::Output { .. }) {
                        return Err(KError::new("E_TEMPLATE_FROZEN", format!("{path}：validation 之后模板集冻结，只能追加输出、收紧字段，不能改已有输出")));
                    }
                }
            }
            touched.push(path.clone());
            current.insert(path.clone(), content.clone());
        }
        // 整体能解析才写盘
        TemplateSet::build(current.into_iter().collect())?;
        for (path, content) in &changes {
            let full = l.store.templates_dir().join(path);
            if let Some(p) = full.parent() {
                std::fs::create_dir_all(p).map_err(crate::store::io_err)?;
            }
            std::fs::write(&full, content).map_err(crate::store::io_err)?;
        }
        l.templates = TemplateSet::read_dir(&l.store.templates_dir())?;
        l.scan.templates.digest = l.templates.digest.clone();
        gates::refresh_all(&l.scan, &mut l.state, &l.templates, l.scan.phase);
        l.scan.updated_at = crate::now();
        l.store.write_scan(&l.scan)?;
        l.store.write_state(&l.state)?;
        let mut v = response(&l.scan, Map::new());
        v["written"] = json!(true);
        v["digest"] = json!(l.templates.digest);
        v["frozen"] = json!(l.scan.templates.frozen);
        v["files"] = json!(touched);
        v["message"] = json!("模板集已登记；记录的警告已按新片段重算");
        Ok(v)
    })
}

// ─── scan_record / scan_progress ─────────────────────────────────────────────

pub fn record(home: &Home, session: &Session, input: &Value) -> KResult<Value> {
    let started = Instant::now();
    let result = (|| {
        let id = input["scanId"].as_str().ok_or_else(|| KError::invalid_input("scanId 必填"))?;
        let ops = input["ops"].as_array().cloned().ok_or_else(|| KError::invalid_input("ops 必填：操作列表"))?;
        let store = ScanStore::open(home.clone(), id)?;
        with_lock(&store, true, || {
            let mut l = load(&store)?;
            if l.scan.sealed_at.is_some() || l.store.is_sealed() {
                return Err(KError::new("E_SEALED", "扫描已封存，不能再写；要改就从它起一次增量扫描"));
            }
            let now = crate::now();
            let outcome = record::apply(&l.store, &mut l.scan, &mut l.state, &l.templates, &l.inventory, &l.pack, &session.id, &ops, &now)?;
            if let Some(from) = outcome.advanced_from {
                log_event(&l.store, Some(session), "gate", "kernel", format!("关卡通过，自动从 {from} 进入 {}", l.scan.phase), json!({ "from": from, "to": l.scan.phase, "declared": false }), vec![]);
            }
            let mut extra = Map::new();
            extra.insert("written".into(), json!(true));
            extra.insert("duplicate".into(), json!(outcome.duplicate));
            extra.insert("ids".into(), json!(outcome.ids));
            extra.insert("warnings".into(), json!(outcome.warnings));
            extra.insert("advancedFrom".into(), json!(outcome.advanced_from));
            extra.insert("gaps".into(), json!(outcome.gaps));
            extra.insert("guidance".into(), json!(text::guidance(l.scan.phase)));
            extra.insert("checkpoint".into(), json!(outcome.checkpoint));
            let message = if outcome.advanced_from.is_some() {
                format!("已记录；关卡通过，进入 {}", l.scan.phase)
            } else if !outcome.gaps.is_empty() {
                format!("已记录；下一关 {} 还差 {} 项，见 gaps", l.scan.phase.next().map(|p| p.to_string()).unwrap_or_default(), outcome.gaps.len())
            } else if !outcome.warnings.is_empty() {
                "已记录，有警告：改了再提交，带警告的记录过不了下一关".to_string()
            } else {
                "已记录".to_string()
            };
            extra.insert("message".into(), json!(message));
            Ok(response(&l.scan, extra))
        })
    })();
    if let Some(id) = input["scanId"].as_str() {
        if let Ok(store) = ScanStore::open(home.clone(), id) {
            log_call(Some(&store), Some(session), "scan_record", input, &result, started);
        }
    }
    result
}

pub fn progress(home: &Home, session: &Session, input: &Value) -> KResult<Value> {
    let started = Instant::now();
    let result = (|| {
        let id = input["scanId"].as_str().ok_or_else(|| KError::invalid_input("scanId 必填"))?;
        let parsed = progress::parse(input)?;
        let store = ScanStore::open(home.clone(), id)?;
        with_lock(&store, true, || {
            let mut l = load(&store)?;
            let now = crate::now();
            let outcome = progress::apply(&l.store, &mut l.scan, &mut l.state, &mut l.progress, &l.templates, &l.inventory, &l.pack, &session.id, &parsed, &now)?;
            let mut extra = Map::new();
            extra.insert("written".into(), json!(true));
            extra.insert("advancedFrom".into(), json!(outcome.advanced_from));
            extra.insert("gaps".into(), json!(outcome.gaps));
            extra.insert("sealed".into(), json!(outcome.sealed));
            extra.insert("outputs".into(), json!(outcome.outputs));
            extra.insert("guidance".into(), json!(text::guidance(l.scan.phase)));
            let message = if outcome.sealed {
                format!("全部关卡通过，已封存；输出：{}", outcome.outputs.join("、"))
            } else if outcome.advanced_from.is_some() {
                format!("关卡通过，进入 {}", l.scan.phase)
            } else if !outcome.gaps.is_empty() {
                format!("阶段 {} 不变：{} 项缺口，见 gaps", l.scan.phase, outcome.gaps.len())
            } else {
                "进度已同步".to_string()
            };
            extra.insert("message".into(), json!(message));
            Ok(response(&l.scan, extra))
        })
    })();
    if let Some(id) = input["scanId"].as_str() {
        if let Ok(store) = ScanStore::open(home.clone(), id) {
            log_call(Some(&store), Some(session), "scan_progress", input, &result, started);
        }
    }
    result
}

// ─── 命令行 ──────────────────────────────────────────────────────────────────

pub fn summary(home: &Home, id: &str) -> KResult<Value> {
    let store = ScanStore::open(home.clone(), id)?;
    with_lock(&store, false, || {
        let l = load(&store)?;
        summary_view(&l)
    })
}

pub fn templates_of(home: &Home, id: &str) -> KResult<Value> {
    let store = ScanStore::open(home.clone(), id)?;
    with_lock(&store, false, || {
        let l = load(&store)?;
        let files: Vec<Value> = l.templates.files.values().map(|f| json!({ "path": f.path, "meta": f.meta.to_value(), "content": f.body })).collect();
        let partials: Vec<Value> = l.templates.partials.iter().map(|(p, c)| json!({ "path": p, "content": c })).collect();
        Ok(json!({ "digest": l.templates.digest, "frozen": l.scan.templates.frozen, "files": files, "partials": partials }))
    })
}

pub fn records(home: &Home, id: &str) -> KResult<Value> {
    let store = ScanStore::open(home.clone(), id)?;
    with_lock(&store, false, || {
        let l = load(&store)?;
        Ok(json!(l.state))
    })
}

pub fn log_entries(home: &Home, id: &str, filter: &LogFilter) -> KResult<Value> {
    let store = ScanStore::open(home.clone(), id)?;
    let entries = with_lock(&store, false, || Ok(log::read_all(&store)))?;
    Ok(json!({ "scanId": id, "entries": log::filter(entries, filter) }))
}

pub fn list(home: &Home, root_filter: Option<&str>, _all: bool) -> KResult<Value> {
    let mut items = Vec::new();
    for (key, id, dir) in home.all_scans() {
        let Ok(scan) = ScanStore::read::<Scan>(&dir.join("scan.json")) else {
            continue;
        };
        if let Some(r) = root_filter {
            let r = PathBuf::from(shellexpand(r));
            let Ok(rc) = r.canonicalize() else { continue };
            if scan.root != rc && !scan.root.starts_with(&rc) {
                continue;
            }
        }
        let prompt = scan.user_requests.first().map(|u| u.text.lines().next().unwrap_or("").to_string()).unwrap_or_default();
        items.push(json!({
            "scanId": id, "targetKey": key, "domain": scan.domain, "phase": scan.phase,
            "sealed": scan.sealed_at.is_some(), "updatedAt": scan.updated_at, "root": scan.root, "prompt": prompt,
        }));
    }
    Ok(json!({ "scans": items }))
}

pub fn delete(home: &Home, id: &str, yes: bool) -> KResult<Value> {
    let (_, dir) = home.find_scan(id).ok_or_else(|| KError::new("E_SCAN_NOT_FOUND", format!("扫描 {id} 不存在")))?;
    if !yes {
        return Err(KError::new("E_CONFIRMATION_REQUIRED", format!("删除 {id} 会连同检查点和日志一起删掉，不可恢复；加 --yes 确认")));
    }
    std::fs::remove_dir_all(&dir).map_err(crate::store::io_err)?;
    Ok(json!({ "ok": true, "deleted": id }))
}

fn read_outputs(sealed: &Path) -> KResult<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = format!("{prefix}{name}");
            if e.path().is_dir() {
                walk(&e.path(), &format!("{rel}/"), out);
            } else if let Ok(b) = std::fs::read(e.path()) {
                out.push((rel, b));
            }
        }
    }
    walk(sealed, "", &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    if out.is_empty() {
        return Err(KError::new("E_NOT_SEALED", "没有封存的输出；先声明 finalized"));
    }
    Ok(out)
}

pub fn render_out(home: &Home, id: &str, out_dir: &Path, force: bool) -> KResult<Value> {
    let store = ScanStore::open(home.clone(), id)?;
    let files = read_outputs(&store.sealed_dir().join("outputs"))?;
    std::fs::create_dir_all(out_dir).map_err(|e| KError::new("E_IO", format!("建目录 {}：{e}", out_dir.display())))?;
    let mut written = Vec::new();
    for (key, bytes) in &files {
        let dst = out_dir.join(key);
        if dst.exists() {
            let same = std::fs::read(&dst).map(|b| b == *bytes).unwrap_or(false);
            if !same && !force {
                return Err(KError::new(
                    "E_OUTPUT_EXISTS",
                    format!("{} 已存在且内容不同；确认后用 --force 覆盖", dst.display()),
                ));
            }
        }
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p).map_err(crate::store::io_err)?;
        }
        std::fs::write(&dst, bytes).map_err(crate::store::io_err)?;
        written.push(key.clone());
    }
    Ok(json!({ "ok": true, "scanId": id, "dir": out_dir, "written": written }))
}

pub fn export(home: &Home, id: &str, file: &Path) -> KResult<Value> {
    let store = ScanStore::open(home.clone(), id)?;
    let l = with_lock(&store, false, || load(&store))?;
    // 导出的和 scans render 写出的来自同一个封存包；没封存就没有包。
    if l.scan.sealed_at.is_none() || !l.store.is_sealed() {
        return Err(KError::new("E_NOT_SEALED", "没有封存包可导出；先声明 finalized"));
    }
    let manifest: Value = ScanStore::read(&store.sealed_dir().join("manifest.json")).unwrap_or(Value::Null);
    let outputs: Vec<Value> = read_outputs(&store.sealed_dir().join("outputs"))
        .unwrap_or_default()
        .into_iter()
        .map(|(k, b)| json!({ "key": k, "content": String::from_utf8_lossy(&b) }))
        .collect();
    let templates: Vec<Value> = l.templates.files.values().map(|f| json!({ "path": f.path, "content": f.body, "meta": f.meta.to_value() })).collect();
    let bundle = json!({
        "manifest": manifest,
        "scan": l.scan,
        "state": l.state,
        "progress": l.progress,
        "templates": { "digest": l.templates.digest, "files": templates, "partials": l.templates.partials },
        "outputs": outputs,
        "log": log::read_all(&store),
    });
    if let Some(p) = file.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p).map_err(crate::store::io_err)?;
        }
    }
    std::fs::write(file, serde_json::to_vec_pretty(&bundle).unwrap_or_default()).map_err(crate::store::io_err)?;
    Ok(json!({ "ok": true, "scanId": id, "file": file }))
}

pub fn info(home: &Home, root: &Path) -> Value {
    let domains: Vec<Value> = pack::describe(home, root);
    let skill = json!({ "name": pack::SKILL_NAME, "description": pack::skill_description() });
    json!({
        "name": "project-scanner",
        "version": crate::VERSION,
        "home": home.root,
        "templateEngine": "minijinja（Jinja2 语法）",
        "anchorFormat": text::ANCHOR_FORMAT,
        "domains": domains,
        "skill": skill,
        "register": {
            "pi": "pi mcp add project-scanner -- project-scanner mcp",
            "mcpJson": { "command": "project-scanner", "args": ["mcp"] },
            "skillsExport": "project-scanner skills export <目录>",
            "packs": "project-scanner packs list | show | check | export",
        },
    })
}

