//! 合成样例：一份覆盖各种记录状态的 state 和一份 scan。给两处用：域包验证时的试渲染
//! （`pack::check`）和渲染测试。不经过内核流程、直接构造，所以能包含封存前不会同时出现的
//! 情况（未结论、搁置、重开）——渲染对任何状态都不能失败。

use crate::model::{Scan, State};
use serde_json::{json, Value};

const REV: &str = "0123456789abcdef0123456789abcdef01234567";
const A: &str = "0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))";

fn section(id: &str, label: &str, kind: &str, paths: &[&str], questions: &[&str]) -> Value {
    json!({
        "id": id, "label": label, "kind": kind, "paths": paths, "questions": questions,
        "disposition": null, "dispositionReason": null,
        "reopened": false, "reopenedAt": null, "mergedInto": null, "mergedAt": null, "abolished": null,
        "warnings": [], "createdAt": "2026-01-01T00:00:00.000Z",
    })
}

fn finding(id: &str, section_id: &str, kind: &str, anchor: &str, status: &str, fields: Value) -> Value {
    json!({
        "id": id, "sectionId": section_id, "kind": kind,
        "identity": {"anchor": anchor, "instance": null},
        "fingerprint": "fp", "anchors": [A], "anchorRefs": [], "status": status, "fields": fields,
        "paths": [], "retractedPaths": [], "warnings": [], "retracted": null,
        "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z",
    })
}

fn path(steps: Value) -> Value {
    json!({ "steps": steps, "prerequisites": ["普通用户可登录"], "warnings": [], "attachedAt": "2026-01-01T00:00:00.000Z" })
}

/// 八个板块（含已合并、已废除、重开）、六条断言（证实带路径和警告、推翻、未结论、已撤回、搁置、流程类）。
pub fn state() -> State {
    let s01 = section("s-01", "认证", "attack-surface", &["src/auth"], &["登录渲染是否转义用户输入", "登出是否清理会话"]);
    let mut validated = finding(
        "f-001",
        "s-01",
        "rce",
        "login-render-without-escaping",
        "validated",
        json!({
            "title": "登录页渲染未转义", "claim": "用户名直接进模板上下文，模板若允许表达式则可执行",
            "verdict": "validated", "rationale": "name 未转义进入 render", "method": "读代码",
            "evidence": [A], "counterevidence": "管理后台同样渲染但需管理员",
            "preconditions": "模板来源可被普通用户影响", "severity": "high", "severityRationale": "普通用户可执行表达式",
            "confidence": "medium", "impact": "普通用户可在服务器上执行任意表达式",
        }),
    );
    validated["identity"]["instance"] = json!("admin");
    validated["anchors"] = json!([A, "0123456789abcdef0123456789abcdef01234567:src/auth/login.ts:block_start(export function logout)"]);
    validated["paths"] = json!([path(json!([
        {"role": "source", "anchor": A, "gap": null, "note": null},
        {"role": "transform", "anchor": null, "gap": "模板引擎内部处理未读到", "note": "框架黑盒"},
        {"role": "sink", "anchor": A, "gap": null, "note": null},
    ]))]);
    validated["warnings"] = json!([
        {"code": "W_ANCHOR_STALE", "field": "anchors[1]", "message": "定位过期：第二个定位所在文件已改", "stage": null},
    ]);

    let mut s03 = section("s-03", "依赖与配置", "baseline", &["package.json"], &["依赖有没有已知漏洞"]);
    s03["disposition"] = json!("no_issue");
    s03["dispositionReason"] = json!("没有第三方依赖");

    let mut s04 = section("s-04", "旧认证", "attack-surface", &["src/old-auth"], &["旧接口是否还在用"]);
    s04["mergedInto"] = json!("s-01");
    let mut s05 = section("s-05", "实验特性", "attack-surface", &["src/exp"], &["实验开关默认是否关闭"]);
    s05["abolished"] = json!({"reason": "代码已删除", "at": "2026-01-01T00:00:00.000Z"});
    let mut s06 = section("s-06", "结账", "attack-surface", &["src/checkout"], &["优惠券叠加能否为负"]);
    s06["reopened"] = json!(true);
    s06["reopenedAt"] = json!("2026-01-01T00:00:00.000Z");

    let mut retracted = finding(
        "f-004",
        "s-01",
        "injection",
        "login-name-sql-concat",
        "retracted",
        json!({"title": "登录名拼 SQL", "claim": "登录名直接拼进查询", "verdict": "validated", "rationale": "看起来像拼接"}),
    );
    retracted["retracted"] = json!({"reason": "实际是参数化查询", "at": "2026-01-01T00:00:00.000Z"});

    let mut flow = finding(
        "f-006",
        "s-08",
        "flow",
        "request-lifecycle",
        "validated",
        json!({
            "title": "请求从入口到响应", "claim": "请求经中间件链到达处理器，再由渲染器输出。",
            "verdict": "validated", "rationale": "沿调用链读完", "confidence": "high",
            "evidence": [A],
        }),
    );
    flow["paths"] = json!([path(json!([
        {"role": "source", "anchor": A, "gap": null, "note": null},
        {"role": "transform", "anchor": A, "gap": null, "note": "中间件"},
        {"role": "sink", "anchor": A, "gap": null, "note": null},
    ]))]);

    serde_json::from_value(json!({
        "schema": 1, "nextSection": 9, "nextFinding": 7,
        "sections": [
            s01,
            section("s-02", "上传", "attack-surface", &["src/upload"], &["上传文件名是否净化"]),
            s03, s04, s05, s06,
            section("s-07", "配置", "baseline", &["config/"], &["调试开关是否默认关闭"]),
            section("s-08", "请求处理流程", "module", &["src/app.ts"], &["一个请求怎么走完"]),
        ],
        "findings": [
            validated,
            finding("f-002", "s-02", "authorization", "upload-path-traversal", "refuted",
                json!({"title": "上传路径穿越", "claim": "文件名未净化", "verdict": "refuted", "rationale": "有 basename 处理"})),
            finding("f-003", "s-06", "injection", "coupon-code-in-template", "candidate",
                json!({"title": "优惠券代码进了模板", "claim": "coupon.code 被当模板渲染"})),
            retracted,
            finding("f-005", "s-07", "config", "debug-endpoint-enabled", "deferred",
                json!({"title": "调试端点默认开启", "claim": "DEBUG 路由没有环境判断", "verdict": "deferred", "rationale": "要确认部署配置"})),
            flow,
        ],
        "exclusions": [
            {"path": "examples/", "reason": "合成示例，不随产品发布", "addedAt": "2026-01-01T00:00:00.000Z"},
            {"path": "vendor/x/", "reason": "上游生成的客户端代码", "addedAt": "2026-01-01T00:00:00.000Z"},
        ],
    }))
    .unwrap()
}

/// 一份已封存的扫描元数据；域按参数填。
pub fn scan(domain: &str) -> Scan {
    serde_json::from_value(json!({
        "schema": 1, "scanId": "aaaaaaaaaaaa", "targetKey": "0123456789abcdef",
        "root": "/srv/app", "remote": "git@example.com:app/app.git", "revision": REV, "dirty": true,
        "snapshotDigest": "d".repeat(64), "domain": domain, "mode": "full", "baseScanId": null,
        "phase": "finalized",
        "writer": {"sessionId": "s", "pid": 1, "since": "2026-01-01T00:00:00.000Z"},
        "userRequests": [
            {"text": "检查这个项目中可能存在并可利用的 bug。", "at": "2026-01-01T00:00:00.000Z", "source": "prompt"},
            {"text": "尤其注意跨租户越权。", "at": "2026-01-01T00:10:00.000Z", "source": "append"},
            {"text": "先不管前端。", "at": "2026-01-01T00:20:00.000Z", "source": "answer"},
        ],
        "policy": {"files": ["SECURITY.md"], "digest": "p".repeat(64)},
        "templates": {"digest": "t".repeat(64), "frozen": true},
        "sealedAt": "2026-01-01T01:00:00.000Z",
        "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:30:00.000Z",
    }))
    .unwrap()
}
