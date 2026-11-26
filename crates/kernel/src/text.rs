//! 固定句式。工具、命令行、将来的界面共用这一处，人和 agent 看到的是同一段。

use crate::model::{Gap, Phase, Warning};

pub const ANCHOR_FORMAT: &str =
    "<commit>[+dirty]:<relpath>:block_start(<line_text>)[:block_end(<line_text>)][:+<offset>]";

fn gap(code: &str, message: String, refs: Vec<String>) -> Gap {
    Gap { code: code.to_string(), message, refs, data: serde_json::Value::Null }
}

fn gap_data(code: &str, message: String, refs: Vec<String>, data: serde_json::Value) -> Gap {
    Gap { code: code.to_string(), message, refs, data }
}

// ─── 错误 ────────────────────────────────────────────────────────────────────

pub fn e_not_writer(since: &str) -> String {
    format!("本会话已不是写者；现在的写者自 {since} 起持有，用 scan_start 恢复以接管")
}

pub fn e_phase_write_forbidden(phase: Phase, op: &str) -> String {
    format!("阶段 {phase} 不允许 {op}")
}

pub fn e_anchor_invalid(reason: &str) -> String {
    format!("定位无效：{reason}。格式：{ANCHOR_FORMAT}")
}

pub fn e_anchor_revision_mismatch(anchor: &str, what: &str) -> String {
    format!("定位 {anchor:?} 的{what}与扫描记录不一致，只能引用扫描时所在的修订")
}

pub fn e_anchor_out_of_scope(anchor: &str) -> String {
    format!("定位 {anchor:?} 指向范围外的文件（不存在、已排除或不是文本文件）")
}

pub fn e_identity_conflict(kind: &str, anchor: &str, id: &str) -> String {
    format!("身份 {kind}/{anchor} 已被 {id} 使用；换 instance 或废除旧的")
}

pub fn e_section_has_findings(label: &str, n: usize) -> String {
    format!("板块 {label} 还有 {n} 条有效断言，先废除或移走")
}

pub fn e_unknown_record(id: &str) -> String {
    format!("记录 {id} 不存在")
}

// ─── 缺口 ────────────────────────────────────────────────────────────────────

pub fn g_plan_empty() -> Gap {
    gap("G_PLAN_EMPTY", "还没有板块：至少一个板块，每个板块有范围和至少一个问题".into(), vec![])
}

pub fn g_section_no_scope(id: &str, label: &str) -> Gap {
    gap("G_SECTION_NO_SCOPE", format!("板块 {label} 缺范围或问题"), vec![id.to_string()])
}

pub fn g_section_uncovered_files(n: usize, sample: &[String]) -> Gap {
    let mut msg = format!("{n} 个文件不属于任何板块");
    if !sample.is_empty() {
        msg.push_str("：");
        msg.push_str(&sample.join("、"));
        if n > sample.len() {
            msg.push_str("…");
        }
    }
    gap_data(
        "G_SECTION_UNCOVERED_FILES",
        msg,
        vec![],
        serde_json::json!({ "count": n, "sample": sample }),
    )
}

pub fn g_section_no_output(id: &str, label: &str) -> Gap {
    gap("G_SECTION_NO_OUTPUT", format!("板块 {label} 没有任何输出接纳它"), vec![id.to_string()])
}

pub fn g_orphan_record(id: &str, label: &str, n: usize) -> Gap {
    let what = if n == 0 { "没有输出接纳它" } else { "有多个输出接纳它，应恰好一个" };
    gap("G_ORPHAN_RECORD", format!("板块 {label} {what}"), vec![id.to_string()])
}

pub fn g_section_no_disposition(id: &str, label: &str) -> Gap {
    gap("G_SECTION_NO_DISPOSITION", format!("板块 {label} 没有断言也没有处置"), vec![id.to_string()])
}

pub fn g_section_investigating(items: &[(String, String, bool)]) -> Gap {
    let names: Vec<String> = items
        .iter()
        .map(|(id, l, reopened)| {
            if *reopened {
                format!("{l}（{id}，重开）")
            } else {
                format!("{l}（{id}）")
            }
        })
        .collect();
    gap(
        "G_SECTION_INVESTIGATING",
        format!("{} 个板块处于调查中：{}", items.len(), names.join("、")),
        items.iter().map(|(id, _, _)| id.clone()).collect(),
    )
}

pub fn g_findings_with_warnings(items: &[(String, Vec<Warning>)]) -> Gap {
    let names: Vec<String> = items
        .iter()
        .map(|(id, ws)| {
            let fields: Vec<String> = ws.iter().map(|w| w.field.clone()).collect();
            format!("{id}（{}）", fields.join("、"))
        })
        .collect();
    gap(
        "G_FINDINGS_WITH_WARNINGS",
        format!("{} 条断言带警告：{}", items.len(), names.join("、")),
        items.iter().map(|(id, _)| id.clone()).collect(),
    )
}

pub fn g_finding_no_verdict(ids: &[String]) -> Gap {
    gap("G_FINDING_NO_VERDICT", format!("{} 条断言没有结论：{}", ids.len(), ids.join("、")), ids.to_vec())
}

pub fn g_path_missing(id: &str, kind: &str) -> Gap {
    gap("G_PATH_MISSING", format!("断言 {id}（{kind}）已证实，需要路径"), vec![id.to_string()])
}

pub fn g_path_incomplete(id: &str, index: usize, what: &str) -> Gap {
    gap("G_PATH_INCOMPLETE", format!("断言 {id} 的路径 {index} 不完整：{what}"), vec![id.to_string()])
}

pub fn g_path_origin_out_of_scope(id: &str, index: usize, relpath: &str) -> Gap {
    gap(
        "G_PATH_ORIGIN_OUT_OF_SCOPE",
        format!("断言 {id} 的路径 {index} 的起点 {relpath} 不在范围内"),
        vec![id.to_string()],
    )
}

pub fn g_render_failed(key: &str, reason: &str) -> Gap {
    gap("G_RENDER_FAILED", format!("输出 {key} 渲染失败：{reason}"), vec![])
}

// ─── 阶段指引 ────────────────────────────────────────────────────────────────

pub fn guidance(phase: Phase) -> &'static str {
    match phase {
        Phase::Preflight => "读上下文，自动进入 planning",
        Phase::Planning => {
            "画项目地图，切板块，每个板块给范围和问题；覆盖由内核计算，未覆盖文件会列在缺口里。计划合法写入后自动进入 discovery"
        }
        Phase::Discovery => {
            "逐板块调查，先存再合并；每条断言带语义锚和定位。全部板块有断言或处置后进入 validation"
        }
        Phase::Validation => {
            "逐条用源码证实或推翻，写结论、证据、反证。新问题先 scan_progress 重开板块再新建断言"
        }
        Phase::AnalyzeFullPath => {
            "给需要路径的断言补完整路径（起点到结果，每步有定位或写明缺口）；然后声明 finalized"
        }
        Phase::Finalized => "已封存。要改就从它起一次增量扫描",
    }
}

pub fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Preflight => "预检",
        Phase::Planning => "计划",
        Phase::Discovery => "调查",
        Phase::Validation => "验证",
        Phase::AnalyzeFullPath => "路径",
        Phase::Finalized => "已封存",
    }
}

/// "下一步"：人转述给 agent 的那段话。
pub fn next_step(phase: Phase, gaps: &[Gap], waiting: Option<&str>) -> String {
    if phase == Phase::Finalized {
        return "已封存。用 scans render 取报告，要改就从它起一次增量扫描".to_string();
    }
    let mut out = String::new();
    if let Some(q) = waiting {
        out.push_str(&format!("等待决定：{q}。回答后 agent 把回答追加到用户要求并清除标记，再继续。"));
    }
    if gaps.is_empty() {
        out.push_str(guidance(phase));
        if phase == Phase::AnalyzeFullPath {
            out.push_str("；没有缺口了就声明 finalized");
        }
        return out;
    }
    out.push_str(&format!("阶段 {}，还差 {} 项：", phase_label(phase), gaps.len()));
    let parts: Vec<String> = gaps.iter().map(|g| g.message.clone()).collect();
    out.push_str(&parts.join("；"));
    out
}
