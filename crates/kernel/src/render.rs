//! 渲染：minijinja，环境固定——没有自动转义，只有 `slug` 一个过滤器，只能 include 模板集里的
//! partials，有步数上限和输出大小上限。渲染上下文由内核构造，顺序按编号。

use crate::model::{Finding, Scan, Section, State};
use crate::pack::Pack;
use crate::status;
use crate::templates::{Meta, TemplateSet};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path};

pub const FUEL: u64 = 1_000_000;
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

/// 小写；不是字母数字的字符换成 `-`；连续的 `-` 合并。
pub fn slug(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() || ch == '.' || ch == '_' {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.extend(ch.to_lowercase());
        } else {
            dash = true;
        }
    }
    if out.is_empty() {
        "section".to_string()
    } else {
        out
    }
}

pub fn section_json(s: &Section, state: &State, with_findings: bool) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(s.id));
    m.insert("label".into(), json!(s.label));
    m.insert("kind".into(), json!(s.kind));
    m.insert("status".into(), json!(status::section_status(s, state).as_str()));
    m.insert("reopened".into(), json!(s.reopened));
    m.insert("paths".into(), json!(s.paths));
    m.insert("questions".into(), json!(s.questions));
    m.insert("disposition".into(), json!(s.disposition));
    m.insert("dispositionReason".into(), json!(s.disposition_reason));
    m.insert("mergedInto".into(), json!(s.merged_into));
    m.insert("abolished".into(), json!(s.abolished));
    m.insert("warnings".into(), json!(s.warnings));
    let findings: Vec<&Finding> = state.findings.iter().filter(|f| f.section_id == s.id).collect();
    let mut counts: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for f in &findings {
        *counts.entry(f.status.as_str().to_string()).or_insert(0) += 1;
    }
    m.insert("findingCounts".into(), json!(counts));
    if with_findings {
        m.insert("findings".into(), json!(findings));
    }
    Value::Object(m)
}

pub struct RenderInput<'a> {
    pub scan: &'a Scan,
    pub state: &'a State,
    pub pack: &'a Pack,
    pub policy_text: Option<&'a str>,
}

/// 渲染上下文：`scan`、`pack`、`userRequests`、`exclusions`、`defaultExclusions`、`sections`、
/// `counts`、`stale`、`retracted`、`policy`。全部来自状态，同一状态渲染结果永远一致。
pub fn build_context(input: &RenderInput<'_>) -> Value {
    let scan = input.scan;
    let state = input.state;
    let pack = input.pack;
    let sections: Vec<Value> = state.sections.iter().filter(|s| s.is_live()).map(|s| section_json(s, state, true)).collect();
    let counts = status::counts(state, pack);
    let stale: Vec<Value> = state
        .findings
        .iter()
        .filter(|f| f.is_effective() && f.warnings.iter().any(|w| w.code == "W_ANCHOR_STALE"))
        .map(|f| {
            json!({
                "findingId": f.id,
                "anchors": f.warnings.iter().filter(|w| w.code == "W_ANCHOR_STALE").map(|w| w.message.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let retracted: Vec<&Finding> = state.findings.iter().filter(|f| !f.is_effective()).collect();
    json!({
        "scan": {
            "id": scan.scan_id,
            "domain": scan.domain,
            "mode": scan.mode,
            "phase": scan.phase,
            "root": scan.root,
            "revision": scan.revision,
            "dirty": scan.dirty,
            "snapshotDigest": scan.snapshot_digest,
            "remote": scan.remote,
            "createdAt": scan.created_at,
            "sealedAt": scan.sealed_at,
            // 封存前用最近更新时间，保证同一状态渲染结果一致；封存时就是封存时间。
            "generatedAt": scan.sealed_at.clone().or(Some(scan.updated_at.clone())),
            "templatesDigest": scan.templates.digest,
            // 域包来源与叠加的层：报告开头写明，读的人知道这份报告按哪套规则来。
            "packSource": scan.pack.source,
            "packOverlays": scan.pack.overlays,
        },
        "pack": {
            "findingKinds": pack.finding_kinds,
            "pathRoles": pack.path_roles,
            "originRoles": pack.origin_roles,
            "outcomeRoles": pack.outcome_roles,
        },
        "userRequests": scan.user_requests,
        "exclusions": state.exclusions,
        "defaultExclusions": pack.default_exclusions,
        "sections": sections,
        "counts": counts,
        "stale": stale,
        "retracted": retracted,
        "policy": input.policy_text,
    })
}

#[derive(Debug, Clone)]
pub struct Rendered {
    pub key: String,
    pub content: String,
}

/// 环境：固定配置。模板全部来自模板集，在内存里登记，渲染不碰文件系统——所以 `include` 只能引用
/// 模板集里的文件。块标签按 Jinja2 的 trim_blocks + lstrip_blocks 处理空白；没有自动转义，只有
/// `slug` 过滤器；有步数上限。片段和输出登记的是去掉 front matter 的正文。
fn env(set: &TemplateSet) -> Result<minijinja::Environment<'static>, String> {
    let mut env = minijinja::Environment::new();
    env.set_syntax(
        minijinja::syntax::SyntaxConfig::builder()
            .trim_blocks(true)
            .lstrip_blocks(true)
            .build()
            .map_err(|e| format!("minijinja 语法配置：{e}"))?,
    );
    env.set_fuel(Some(FUEL));
    env.add_filter("slug", slug);
    for (path, f) in &set.files {
        env.add_template_owned(path.clone(), f.body.clone()).map_err(|e| format!("{path}: {e}"))?;
    }
    for (path, content) in &set.partials {
        env.add_template_owned(path.clone(), content.clone()).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(env)
}

fn render_one(env: &minijinja::Environment<'_>, name: &str, ctx: &Value) -> Result<String, String> {
    let tmpl = env.get_template(name).map_err(|e| format!("{name}: {e}"))?;
    let out = tmpl.render(minijinja::Value::from(minijinja::value::Serde(ctx))).map_err(|e| format!("{name}: {e}"))?;
    if out.len() > MAX_BYTES {
        return Err(format!("{name}: 输出超过 {MAX_BYTES} 字节"));
    }
    // 文本文件约定：结尾恰好一个换行，不依赖模板作者（minijinja 默认会去掉模板末尾的换行）。
    let out = if out.trim_end().is_empty() { out } else { format!("{}\n", out.trim_end_matches('\n')) };
    Ok(out)
}

fn check_key(key: &str) -> Result<(), String> {
    let p = Path::new(key);
    if key.trim().is_empty() || p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("输出的 key 不合法：{key:?}"));
    }
    Ok(())
}

/// 渲染全部输出。`all` 一份；`section` 每个活着的板块一份（key 是 Jinja，可用 `section.label`、
/// `section.id`、`scan.id`、`scan.domain` 和 `slug`）；`sections` 一份导航页（不带断言）。
pub fn render_all(set: &TemplateSet, input: &RenderInput<'_>) -> Result<Vec<Rendered>, String> {
    let env = env(set)?;
    let ctx = build_context(input);
    let mut out = Vec::new();
    for o in set.outputs() {
        let Meta::Output { key, select } = &o.meta else { continue };
        match select.as_str() {
            "all" => {
                check_key(key)?;
                out.push(Rendered { key: key.clone(), content: render_one(&env, &o.path, &ctx)? });
            }
            "sections" => {
                check_key(key)?;
                let mut c = ctx.clone();
                let nav: Vec<Value> = input.state.sections.iter().filter(|s| s.is_live()).map(|s| section_json(s, input.state, false)).collect();
                c["sections"] = Value::Array(nav);
                out.push(Rendered { key: key.clone(), content: render_one(&env, &o.path, &c)? });
            }
            "section" => {
                let mut used: HashSet<String> = HashSet::new();
                for s in input.state.sections.iter().filter(|s| s.is_live()) {
                    let kctx = json!({
                        "scan": { "id": input.scan.scan_id, "domain": input.scan.domain },
                        "section": { "id": s.id, "label": s.label },
                    });
                    let mut key = env.render_str(key, minijinja::Value::from(minijinja::value::Serde(&kctx))).map_err(|e| format!("{} 的 key：{e}", o.path))?;
                    check_key(&key)?;
                    if !used.insert(key.clone()) {
                        key = match key.rfind('.') {
                            Some(i) => format!("{}-{}{}", &key[..i], s.id, &key[i..]),
                            None => format!("{key}-{}", s.id),
                        };
                        used.insert(key.clone());
                    }
                    let mut c = ctx.clone();
                    c["section"] = section_json(s, input.state, true);
                    c["sections"] = json!([c["section"].clone()]);
                    out.push(Rendered { key, content: render_one(&env, &o.path, &c)? });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

pub fn write_outputs(dir: &Path, rendered: &[Rendered]) -> Result<(), String> {
    for r in rendered {
        check_key(&r.key)?;
        let path = dir.join(&r.key);
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        fs::write(&path, &r.content).map_err(|e| format!("写 {}: {e}", path.display()))?;
    }
    Ok(())
}
