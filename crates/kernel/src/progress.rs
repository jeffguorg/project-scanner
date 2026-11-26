//! scan_progress：进度、追加用户要求、等待决定、重开板块、声明推进（含封存）。

use crate::error::{KError, KResult};
use crate::gates::{self, GateEnv};
use crate::inventory::Inventory;
use crate::log;
use crate::model::*;
use crate::pack::Pack;
use crate::render::{self, RenderInput};
use crate::store::ScanStore;
use crate::templates::TemplateSet;
use crate::text;
use serde_json::{json, Value};

#[derive(Debug, Clone, Default)]
pub struct ProgressInput {
    pub active_sections: Option<Vec<String>>,
    pub note: Option<String>,
    pub append_request: Option<String>,
    /// None：没提；Some(None)：清除；Some(Some(q))：标记。
    pub waiting: Option<Option<String>>,
    pub reopen: Vec<String>,
    pub advance: Option<Phase>,
}

pub fn parse(v: &Value) -> KResult<ProgressInput> {
    let Value::Object(m) = v else {
        return Err(KError::invalid_input("输入应是对象"));
    };
    let err = |what: &str| KError::invalid_input(format!("scan_progress：{what}"));
    let mut out = ProgressInput::default();
    match m.get("activeSections") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            let mut ids = Vec::new();
            for x in a {
                match x {
                    Value::String(s) if !s.trim().is_empty() => ids.push(s.clone()),
                    _ => return Err(err("activeSections 的每一项应为板块标识")),
                }
            }
            out.active_sections = Some(ids);
        }
        _ => return Err(err("activeSections 应为板块标识列表")),
    }
    match m.get("note") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) => out.note = Some(s.clone()),
        _ => return Err(err("note 应为字符串")),
    }
    match m.get("appendRequest") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if !s.trim().is_empty() => out.append_request = Some(s.clone()),
        _ => return Err(err("appendRequest 应为非空字符串")),
    }
    out.waiting = match m.get("waiting") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::Object(w)) => match w.get("question") {
            Some(Value::String(q)) if !q.trim().is_empty() => Some(Some(q.clone())),
            _ => return Err(err("waiting 应为 { question } 或 null")),
        },
        _ => return Err(err("waiting 应为 { question } 或 null")),
    };
    match m.get("reopen") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for x in a {
                match x {
                    Value::String(s) if !s.trim().is_empty() => out.reopen.push(s.clone()),
                    _ => return Err(err("reopen 的每一项应为板块标识")),
                }
            }
        }
        _ => return Err(err("reopen 应为板块标识列表")),
    }
    match m.get("advance") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) => {
            out.advance = Some(Phase::parse(s).ok_or_else(|| err(&format!("advance {s:?} 不是阶段名")))?);
        }
        _ => return Err(err("advance 应为阶段名")),
    }
    Ok(out)
}

/// 封存：渲染全部输出，写 manifest、records、templates、log。之后不可改。
pub fn seal(store: &ScanStore, scan: &mut Scan, state: &State, templates: &TemplateSet, pack: &Pack, now: &str) -> KResult<Vec<String>> {
    let policy_text = std::fs::read_to_string(store.policy_path()).ok();
    // 封存时间先写进去，报告里的“生成于”就是它。
    scan.sealed_at = Some(now.to_string());
    let input = RenderInput { scan, state, pack, policy_text: policy_text.as_deref() };
    let rendered = render::render_all(templates, &input)
        .map_err(|e| KError::new("E_RENDER_FAILED", format!("渲染失败，不封存：{e}")))?;
    let sealed = store.sealed_dir();
    render::write_outputs(&sealed.join("outputs"), &rendered).map_err(|e| KError::new("E_IO", e))?;
    let keys: Vec<String> = rendered.iter().map(|r| r.key.clone()).collect();
    let manifest = json!({
        "schema": SCHEMA,
        "scanId": scan.scan_id,
        "targetKey": scan.target_key,
        "root": scan.root,
        "remote": scan.remote,
        "revision": scan.revision,
        "dirty": scan.dirty,
        "snapshotDigest": scan.snapshot_digest,
        "domain": scan.domain,
        "mode": scan.mode,
        "userRequests": scan.user_requests,
        "exclusions": state.exclusions,
        "defaultExclusions": pack.default_exclusions,
        "policy": scan.policy,
        "templatesDigest": templates.digest,
        "counts": crate::status::counts(state, pack),
        "outputs": keys,
        "createdAt": scan.created_at,
        "sealedAt": now,
    });
    ScanStore::write_json(&sealed.join("manifest.json"), &manifest).map_err(crate::store::io_err)?;
    ScanStore::write_json(&sealed.join("records.json"), state).map_err(crate::store::io_err)?;
    copy_dir(&store.templates_dir(), &sealed.join("templates"))?;
    if store.log_path().exists() {
        std::fs::copy(store.log_path(), sealed.join("log.jsonl")).map_err(crate::store::io_err)?;
    }
    scan.phase = Phase::Finalized;
    Ok(keys)
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> KResult<()> {
    if !from.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(to).map_err(crate::store::io_err)?;
    for e in std::fs::read_dir(from).map_err(crate::store::io_err)? {
        let e = e.map_err(crate::store::io_err)?;
        let dst = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), dst).map_err(crate::store::io_err)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub written: bool,
    pub phase: Phase,
    pub advanced_from: Option<Phase>,
    pub gaps: Vec<Gap>,
    pub sealed: bool,
    pub outputs: Vec<String>,
}

pub fn apply(
    store: &ScanStore,
    scan: &mut Scan,
    state: &mut State,
    progress: &mut Progress,
    templates: &TemplateSet,
    inventory: &Inventory,
    pack: &Pack,
    session_id: &str,
    input: &ProgressInput,
    now: &str,
) -> KResult<Outcome> {
    crate::store::writer_check(scan, session_id)?;
    if scan.sealed_at.is_some() || store.is_sealed() {
        return Err(KError::new("E_SEALED", "扫描已封存，不能再写；要改就从它起一次增量扫描"));
    }
    let mut touched = false;

    if !input.reopen.is_empty() {
        if scan.phase < Phase::Discovery {
            return Err(KError::new("E_PHASE_WRITE_FORBIDDEN", text::e_phase_write_forbidden(scan.phase, "reopen")));
        }
        for id in &input.reopen {
            let i = state.section_index(id).ok_or_else(|| KError::new("E_UNKNOWN_RECORD", text::e_unknown_record(id)))?;
            if !state.sections[i].is_live() {
                return Err(KError::new("E_UNKNOWN_RECORD", format!("板块 {id} 已并入或废除，不能重开")));
            }
            if !state.sections[i].reopened {
                state.sections[i].reopened = true;
                state.sections[i].reopened_at = Some(now.to_string());
            }
        }
        log::append(
            store,
            log::LogEntry {
                seq: 0,
                at: now.to_string(),
                kind: "event".into(),
                writer: Some(session_id.to_string()),
                event: Some("reopen".into()),
                source: Some("mcp".into()),
                detail: json!({ "sections": input.reopen }),
                ..Default::default()
            },
        )
        .map_err(crate::store::io_err)?;
    }

    if let Some(ids) = &input.active_sections {
        for id in ids {
            let i = state.section_index(id).ok_or_else(|| KError::new("E_UNKNOWN_RECORD", text::e_unknown_record(id)))?;
            if !state.sections[i].is_live() {
                return Err(KError::new("E_UNKNOWN_RECORD", format!("板块 {id} 已并入或废除，不能标为正在处理")));
            }
        }
        progress.active_sections = ids.clone();
        touched = true;
    }
    if input.note.is_some() {
        progress.note = input.note.clone();
        touched = true;
    }
    if let Some(q) = &input.append_request {
        scan.user_requests.push(UserRequest { text: q.clone(), at: now.to_string(), source: "append".into() });
        touched = true;
    }
    if let Some(w) = &input.waiting {
        progress.waiting_for_decision = w.clone();
        touched = true;
    }

    let mut advanced_from = None;
    let mut gaps = Vec::new();
    let mut sealed_keys = Vec::new();
    if let Some(target) = input.advance {
        if scan.phase.next() != Some(target) {
            return Err(KError::new(
                "E_INVALID_ADVANCE",
                format!("现在在 {}，只能声明进入 {}；阶段只升不降", scan.phase, scan.phase.next().map(|p| p.to_string()).unwrap_or_else(|| "无".into())),
            ));
        }
        let mut probe = state.clone();
        gates::refresh_all(scan, &mut probe, templates, target);
        let policy_text = std::fs::read_to_string(store.policy_path()).ok();
        let env = GateEnv {
            scan,
            inventory,
            pack,
            templates,
            policy_text: policy_text.as_deref(),
        };
        gaps = gates::evaluate(target, &probe, &env);
        if gaps.is_empty() {
            advanced_from = Some(scan.phase);
            *state = probe;
            scan.phase = target;
            if target == Phase::Validation {
                scan.templates.frozen = true;
            }
            if target == Phase::Finalized {
                sealed_keys = seal(store, scan, state, templates, pack, now)?;
            }
        }
        log::append(
            store,
            log::LogEntry {
                seq: 0,
                at: now.to_string(),
                kind: "event".into(),
                writer: Some(session_id.to_string()),
                event: Some(if sealed_keys.is_empty() { "gate".into() } else { "seal".into() }),
                source: Some("kernel".into()),
                message: Some(if gaps.is_empty() { format!("声明进入 {target}：通过") } else { format!("声明进入 {target}：未通过，{} 项缺口", gaps.len()) }),
                gaps: gaps.clone(),
                detail: json!({ "declared": true, "from": advanced_from, "to": target }),
                ..Default::default()
            },
        )
        .map_err(crate::store::io_err)?;
        touched = true;
    }

    if !touched && input.reopen.is_empty() {
        return Err(KError::invalid_input("scan_progress 没有要做的事：给 activeSections、note、appendRequest、waiting、reopen 或 advance"));
    }
    progress.updated_at = Some(now.to_string());
    scan.updated_at = now.to_string();
    store.write_progress(progress)?;
    store.write_scan(scan)?;
    store.write_state(state)?;
    Ok(Outcome { written: true, phase: scan.phase, advanced_from, gaps, sealed: !sealed_keys.is_empty(), outputs: sealed_keys })
}
