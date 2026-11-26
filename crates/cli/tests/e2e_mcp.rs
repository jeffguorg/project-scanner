//! 端到端：起真实的 `project-scanner mcp`（stdio），按 JSON-RPC 把五个工具走一遍直到封存，再用 CLI 取结果。
//!
//! 覆盖：自动推进与声明推进；三种写后果（结构性错误不写、记录警告随记录、关卡拒绝）；重开；
//! 写者接管（旧会话 `E_NOT_WRITER`）；同样内容重提的幂等；渲染冲突与 `--force`；导出。
//! 仓库是一个临时 git 工作区，带未提交改动，所以定位都带 `+dirty`。

use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

const BIN: &str = env!("CARGO_BIN_EXE_project-scanner");
const PROMPT: &str = "检查这个项目中可能存在并可利用的 bug，尤其注意远程代码执行。";

// ─── MCP 客户端 ──────────────────────────────────────────────────────────────

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn start(home: &Path) -> Mcp {
        let mut child = Command::new(BIN)
            .arg("--home")
            .arg(home)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("起 project-scanner mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut m = Mcp { child, stdin, stdout, next_id: 1 };
        let init = m.rpc(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "0"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "project-scanner", "{init}");
        assert!(init["result"]["instructions"].as_str().is_some_and(|s| !s.is_empty()), "server 要带 instructions");
        m.notify("notifications/initialized");
        m
    }

    fn send(&mut self, msg: &Value) {
        self.stdin.write_all(msg.to_string().as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).unwrap();
            assert!(n > 0, "MCP 服务在 {method} 之前退出了");
            let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("不是 JSON：{e}：{line}"));
            if v["id"].as_u64() == Some(id) {
                return v;
            }
        }
    }

    /// 调工具，返回 structuredContent；协议层错误原样返回。
    fn call(&mut self, tool: &str, args: Value) -> Value {
        let r = self.rpc("tools/call", json!({"name": tool, "arguments": args}));
        if r.get("error").is_some() {
            return json!({"protocolError": r["error"]});
        }
        match &r["result"]["structuredContent"] {
            Value::Null => json!({"isError": r["result"]["isError"], "content": r["result"]["content"]}),
            sc => sc.clone(),
        }
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ─── 样例仓库与辅助 ──────────────────────────────────────────────────────────

struct Fixture {
    home: PathBuf,
    repo: PathBuf,
    rev: String,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} 失败：{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn fixture() -> Fixture {
    let base = std::env::temp_dir().join(format!(
        "pscan-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let home = base.join("home");
    let repo = base.join("repo");
    fs::create_dir_all(repo.join("src/auth")).unwrap();
    fs::create_dir_all(repo.join("src/checkout")).unwrap();
    fs::create_dir_all(repo.join("examples")).unwrap();
    fs::write(repo.join("SECURITY.md"), "# 策略\n\n- exclude: examples/\n\n什么重要：会话\n").unwrap();
    fs::write(
        repo.join("src/auth/login.ts"),
        "import { render } from '../render';\n\nexport function login(req, res) {\n  const name = req.body.name;\n  return res.render('login', { name });\n}\n\nexport function logout(req, res) {\n  return res.redirect('/');\n}\n",
    )
    .unwrap();
    fs::write(
        repo.join("src/checkout/total.py"),
        "def total(items, coupon):\n    subtotal = sum(i['price'] for i in items)\n    if coupon:\n        subtotal -= coupon.value\n    return max(subtotal, 0)\n",
    )
    .unwrap();
    fs::write(repo.join("examples/demo.ts"), "export const demo = 1;\n").unwrap();
    fs::write(repo.join("package.json"), "{\"name\":\"fixture\"}\n").unwrap();
    fs::write(repo.join("logo.png"), b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    // 提交之后再改一行：工作区是脏的，测试 +dirty 的定位。
    let login = repo.join("src/auth/login.ts");
    let mut text = fs::read_to_string(&login).unwrap();
    text.push_str("// 未提交的改动\n");
    fs::write(&login, text).unwrap();
    let rev = git(&repo, &["rev-parse", "HEAD"]);
    Fixture { home, repo, rev }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn codes(v: &Value) -> Vec<String> {
    v["gaps"].as_array().map(|g| g.iter().map(|x| x["code"].as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

/// 结构性错误：ok=false，带代码和消息，什么都没写。
fn expect_err(v: &Value, code: &str) {
    assert_eq!(v["ok"], false, "应当是结构性错误 {code}：{v}");
    assert_eq!(s(v, "code"), code, "{v}");
    assert!(!s(v, "message").is_empty(), "{v}");
}

fn cli(home: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(BIN).arg("--home").arg(home).args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn cli_home(f: &Fixture) -> PathBuf {
    f.home.clone()
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[test]
fn full_scan_over_real_mcp_until_sealed() {
    let f = fixture();
    let home = cli_home(&f);
    let mut a = Mcp::start(&home);
    let a1 = format!("{}+dirty:src/auth/login.ts:block_start(res.render('login', {{ name }}))", f.rev);
    let a2 = format!("{}+dirty:src/auth/login.ts:block_start(export function login):block_end(return res.render):+2", f.rev);
    let a3 = format!("{}+dirty:src/checkout/total.py:block_start(subtotal -= coupon.value)", f.rev);
    let a4 = format!("{}+dirty:src/auth/login.ts:block_start(return res.redirect('/'))", f.rev);
    let a5 = format!("{}+dirty:src/auth/login.ts:block_start(const name = req.body.name)", f.rev);

    let tools = a.rpc("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["scan_context", "scan_progress", "scan_record", "scan_start", "scan_templates"]);

    // preflight：读 git、清点、记下用户原话。
    let v = a.call("scan_start", json!({"root": f.repo, "domain": "security", "prompt": PROMPT}));
    assert_eq!(s(&v["scan"], "phase"), "preflight", "{v}");
    assert_eq!(v["scan"]["dirty"], true, "{v}");
    assert_eq!(v["scan"]["revision"], f.rev);
    let sid = s(&v["scan"], "scanId").to_string();
    assert_eq!(sid.len(), 12);

    // 再起一个同根的扫描：允许多次扫描并存，各按 id 区分。
    let again = a.call("scan_start", json!({"root": f.repo, "domain": "security", "prompt": PROMPT}));
    assert_eq!(again["ok"], true, "{again}");
    assert_ne!(s(&again["scan"], "scanId"), sid);

    // planning：上下文带片段的字段表、策略原文、缺口。
    let v = a.call("scan_context", json!({"scanId": sid}));
    assert_eq!(s(&v["scan"], "phase"), "planning", "{v}");
    assert_eq!(codes(&v), ["G_PLAN_EMPTY"]);
    assert_eq!(v["userRequests"][0]["text"], PROMPT);
    assert_eq!(v["userRequests"][0]["source"], "prompt");
    assert_eq!(v["policy"], "# 策略\n\n- exclude: examples/\n\n什么重要：会话\n");
    assert_eq!(v["fragments"]["section"]["fields"]["label"]["max"], 80.0);
    let v = a.call("scan_context", json!({"scanId": sid, "view": "templates"}));
    assert_eq!(v["frozen"], false, "{v}");
    assert!(v["files"].as_array().is_some_and(|a| a.iter().any(|f| f["path"] == "outputs/REPORT.md")), "{v}");

    // 模板只能追加和收紧：收紧（多要一个必填字段）可以，再放宽回去不行。
    let exclusion = "---
type: fragment
select: { record: exclusion, stage: planning }
fields:
  path: { type: text, required: true }
  reason: { type: text, required: true, max: 500 }
---
- `{{ path }}`：{{ reason }}`
";
    let tightened = exclusion.replace("  reason:", "  expire: { type: text, required: true, max: 40 }\n  reason:");
    let v = a.call("scan_templates", json!({"scanId": sid, "tighten": [{"path": "fragments/exclusion.planning.md", "content": tightened}]}));
    assert_eq!(v["written"], true, "{v}");
    let v = a.call("scan_templates", json!({"scanId": sid, "tighten": [{"path": "fragments/exclusion.planning.md", "content": exclusion}]}));
    expect_err(&v, "E_TEMPLATE_LOOSENED");
    // 追加一个输出；之后上下文里的字段表就是收紧后的。
    let summary = "---\ntype: output\nkey: SUMMARY.md\nselect: sections\n---\n# 摘要\n\n- 证实 {{ counts.findings[\"validated\"] | default(0) }} 条\n";
    let v = a.call("scan_templates", json!({"scanId": sid, "add": [{"path": "outputs/SUMMARY.md", "content": summary}]}));
    assert_eq!(v["written"], true, "{v}");
    let v = a.call("scan_context", json!({"scanId": sid, "view": "templates"}));
    assert!(v["files"].as_array().is_some_and(|a| a.iter().any(|f| f["path"] == "outputs/SUMMARY.md")), "{v}");
    let v = a.call("scan_context", json!({"scanId": sid}));
    assert_eq!(v["fragments"]["exclusion"]["fields"]["expire"]["required"], true, "{v}");

    // 计划：两个板块；examples/ 被策略排除，策略文件自己不算未覆盖。
    let v = a.call("scan_record", json!({"scanId": sid, "ops": [
        {"op": "section.create", "label": "认证", "kind": "attack-surface", "paths": ["src/auth"], "questions": ["登录渲染是否转义用户输入", "登出是否清理会话"]},
        {"op": "section.create", "label": "结账", "kind": "attack-surface", "paths": ["src/checkout"], "questions": ["优惠券叠加能否导致负数金额"]},
    ]}));
    assert_eq!(v["ids"], json!(["s-01", "s-02"]), "{v}");
    assert_eq!(s(&v, "phase"), "planning");
    assert_eq!(codes(&v), ["G_SECTION_UNCOVERED_FILES"]);
    assert_eq!(v["gaps"][0]["data"]["sample"], json!(["package.json"]), "{}", v["gaps"][0]);
    assert_eq!(v["message"], "已记录；下一关 discovery 还差 1 项，见 gaps");

    // 结构性错误：整批不写。
    let v = a.call("scan_record", json!({"scanId": sid, "ops": [{"op": "section.create", "label": "依赖", "kind": "baseline", "paths": ["package.json"]}]}));
    expect_err(&v, "E_INVALID_INPUT");
    let v = a.call("scan_context", json!({"scanId": sid}));
    assert_eq!(v["counts"]["inventory"]["uncovered"], 1, "出错的那批不该写进去");

    // 补齐计划：关卡通过，自动进 discovery。
    let v = a.call("scan_record", json!({"scanId": sid, "ops": [{"op": "section.create", "label": "依赖与配置", "kind": "baseline", "paths": ["package.json"], "questions": ["依赖有没有已知漏洞"]}]}));
    assert_eq!(s(&v, "phase"), "discovery", "{v}");
    assert_eq!(s(&v, "advancedFrom"), "planning");

    // 定位：修订要对得上，行文本要在文件里唯一。
    let bad = |anchor: String| {
        json!({"scanId": sid, "ops": [{"op": "finding.create", "sectionId": "s-01", "kind": "rce",
            "identity": {"anchor": "login-render-without-escaping"}, "anchors": [anchor], "fields": {"title": "t", "claim": "c"}}]})
    };
    expect_err(&a.call("scan_record", bad("deadbeef:src/auth/login.ts:block_start(res.render('login', { name }))".into())), "E_ANCHOR_REVISION_MISMATCH");
    expect_err(&a.call("scan_record", bad(format!("{}+dirty:src/auth/login.ts:block_start(no-such-line)", f.rev))), "E_ANCHOR_INVALID");
    expect_err(&a.call("scan_record", bad(format!("{}+dirty:src/checkout/total.py:block_start(subtotal)", f.rev))), "E_ANCHOR_INVALID");

    // 断言：f-002 缺标题，是警告不是错误；警告随记录返回，也挡住下一关。
    let v = a.call("scan_record", json!({"scanId": sid, "ops": [
        {"op": "finding.create", "sectionId": "s-01", "kind": "rce", "identity": {"anchor": "login-render-without-escaping"}, "anchors": [a1, a2], "fields": {"title": "登录页渲染未转义", "claim": "用户名直接进模板上下文，模板若允许表达式则可执行"}},
        {"op": "finding.create", "sectionId": "s-02", "kind": "authorization", "identity": {"anchor": "coupon-negative-total"}, "anchors": [a3], "fields": {"title": "", "claim": "优惠券叠加可以为负"}},
    ]}));
    assert_eq!(v["ids"], json!(["f-001", "f-002"]), "{v}");
    assert_eq!(v["warnings"][0]["recordId"], "f-002");
    assert_eq!(v["warnings"][0]["warnings"][0]["code"], "W_REQUIRED_MISSING");
    let want: Vec<String> = ["G_FINDINGS_WITH_WARNINGS", "G_SECTION_INVESTIGATING", "G_SECTION_NO_DISPOSITION"].iter().map(|x| x.to_string()).collect();
    let mut got = codes(&v);
    got.sort();
    assert_eq!(got, want, "{v}");

    // 身份冲突。
    expect_err(&a.call("scan_record", bad(a1.clone())), "E_IDENTITY_CONFLICT");

    // 声明进 validation：没处置、有警告，拒绝；阶段不变。
    let v = a.call("scan_progress", json!({"scanId": sid, "advance": "validation"}));
    assert_eq!(s(&v, "phase"), "discovery", "{v}");
    got = codes(&v);
    got.sort();
    assert_eq!(got, want, "{v}");
    assert_eq!(v["ok"], true);

    // 补齐：标题、结论、处置。自动进 validation，模板集冻结。
    let v = a.call("scan_record", json!({"scanId": sid, "ops": [
        {"op": "finding.update", "id": "f-002", "fields": {"title": "优惠券叠加导致负数金额"}},
        {"op": "verdict.attach", "id": "f-001", "fields": {"verdict": "validated", "rationale": "name 未转义进入 render", "method": "读代码", "evidence": [a1], "counterevidence": "管理后台同样渲染但需管理员", "preconditions": "模板来源可被普通用户影响", "severity": "high", "severityRationale": "普通用户可执行表达式", "confidence": "medium"}},
        {"op": "verdict.attach", "id": "f-002", "fields": {"verdict": "refuted", "rationale": "有 max(subtotal, 0) 兜底"}},
        {"op": "disposition.set", "sectionId": "s-03", "disposition": "no_issue", "reason": "没有依赖"},
    ]}));
    assert_eq!(s(&v, "phase"), "validation", "{v}");
    assert_eq!(s(&v, "advancedFrom"), "discovery");
    // 模板集冻结：可以追加输出，不能改已有输出。
    let v = a.call("scan_templates", json!({"scanId": sid, "add": [{"path": "outputs/EXTRA.md", "content": "---\ntype: output\nkey: EXTRA.md\nselect: sections\n---\n补充说明\n"}]}));
    assert_eq!(v["frozen"], true, "{v}");
    let v = a.call("scan_templates", json!({"scanId": sid, "tighten": [{"path": "outputs/SUMMARY.md", "content": "---\ntype: output\nkey: SUMMARY.md\nselect: sections\n---\n改过的摘要\n"}]}));
    expect_err(&v, "E_TEMPLATE_FROZEN");

    // validation 里不能新建断言。
    let v = a.call("scan_record", bad(a1.clone()));
    expect_err(&v, "E_PHASE_WRITE_FORBIDDEN");

    // 重开是唯一的出口：重开后板块处于调查中，声明被拒。
    let v = a.call("scan_progress", json!({"scanId": sid, "reopen": ["s-01"], "advance": "analyze_full_path"}));
    assert_eq!(s(&v, "phase"), "validation", "{v}");
    assert_eq!(codes(&v), ["G_SECTION_INVESTIGATING", "G_FINDINGS_WITH_WARNINGS"], "{v}");
    let v = a.call("scan_record", json!({"scanId": sid, "ops": [
        {"op": "finding.create", "sectionId": "s-01", "kind": "disclosure", "identity": {"anchor": "logout-open-redirect"}, "anchors": [a4], "fields": {"title": "登出重定向", "claim": "重定向到固定路径，无风险"}},
        {"op": "verdict.attach", "id": "f-003", "fields": {"verdict": "refuted", "rationale": "目标是常量"}},
    ]}));
    assert_eq!(v["ids"], json!(["f-003"]), "{v}");
    assert_eq!(s(&v, "phase"), "validation", "{v}");
    assert_eq!(v["gaps"][0]["code"], "G_FINDINGS_WITH_WARNINGS", "{v}");
    assert!(s(&v["gaps"][0], "message").contains("impact"), "{}", v["gaps"][0]);

    // 进度：留个便签、标记等决定、再清掉。
    let v = a.call("scan_progress", json!({"scanId": sid, "note": "还差 f-001 的影响面", "waiting": {"question": "要不要把管理后台算进范围？"}}));
    assert_eq!(v["ok"], true, "{v}");
    let v = a.call("scan_context", json!({"scanId": sid}));
    assert_eq!(v["progress"]["note"], "还差 f-001 的影响面");
    assert_eq!(v["progress"]["waitingForDecision"], "要不要把管理后台算进范围？");
    let v = a.call("scan_progress", json!({"scanId": sid, "waiting": null}));
    assert!(v["progress"]["waitingForDecision"].is_null(), "{v}");

    // 写者接管：新会话 resume 拿到写权，旧会话再写就是 E_NOT_WRITER。
    let mut b = Mcp::start(&home);
    let v = b.call("scan_start", json!({"resume": sid}));
    assert_eq!(v["resumed"], true, "{v}");
    assert_eq!(v["sealed"], false);
    let v = a.call("scan_record", json!({"scanId": sid, "ops": [{"op": "finding.update", "id": "f-001", "fields": {"impact": "x"}}]}));
    expect_err(&v, "E_NOT_WRITER");
    let v = a.call("scan_progress", json!({"scanId": sid, "advance": "analyze_full_path"}));
    expect_err(&v, "E_NOT_WRITER");

    // 补影响面：自动进 analyze_full_path。
    let v = b.call("scan_record", json!({"scanId": sid, "ops": [{"op": "finding.update", "id": "f-001", "fields": {"impact": "普通用户可在服务器上执行任意表达式，等同远程代码执行"}}]}));
    assert_eq!(s(&v, "phase"), "analyze_full_path", "{v}");
    assert_eq!(s(&v, "advancedFrom"), "validation");

    // 声明 finalized：缺路径，拒绝。
    let v = b.call("scan_progress", json!({"scanId": sid, "advance": "finalized"}));
    assert_eq!(s(&v, "phase"), "analyze_full_path", "{v}");
    assert_eq!(codes(&v), ["G_PATH_MISSING"]);

    // 路径：中间一步是带理由的缺口，算完整。
    let ops = json!([{"op": "path.attach", "id": "f-001", "steps": [
        {"role": "source", "anchor": a5},
        {"role": "transform", "gap": "模板引擎内部处理未读到", "note": "框架黑盒"},
        {"role": "sink", "anchor": a1},
    ], "prerequisites": ["普通用户可登录"]}]);
    let v = b.call("scan_record", json!({"scanId": sid, "ops": ops}));
    assert_eq!(v["ids"], json!(["f-001"]), "{v}");
    assert_eq!(codes(&v), Vec::<String>::new(), "{v}");
    // 同样内容再提一次：幂等，不重复写。
    let v = b.call("scan_record", json!({"scanId": sid, "ops": ops}));
    assert_eq!(v["duplicate"], true, "{v}");
    assert_eq!(v["ids"], json!(["f-001"]));

    // 封存。
    let v = b.call("scan_progress", json!({"scanId": sid, "advance": "finalized"}));
    assert_eq!(s(&v, "phase"), "finalized", "{v}");
    assert_eq!(v["sealed"], true, "{v}");
    assert_eq!(v["outputs"], json!(["EXTRA.md", "REPORT.md", "SUMMARY.md"]));
    // 封存后只读。
    let v = b.call("scan_record", json!({"scanId": sid, "ops": [{"op": "finding.update", "id": "f-001", "fields": {"impact": "再改"}}]}));
    expect_err(&v, "E_SEALED");
    let v = b.call("scan_start", json!({"resume": sid}));
    assert_eq!(v["sealed"], true, "{v}");
    drop(b);
    drop(a);

    // ─── CLI ────────────────────────────────────────────────────────────────

    let (ok, out, err) = cli(&home, &["scans", "list"]);
    assert!(ok, "{err}");
    assert!(out.contains(&sid) && out.contains("security"), "{out}");
    let (ok, out, err) = cli(&home, &["scans", "show", &sid]);
    assert!(ok, "{err}");
    assert!(out.contains("finalized（已封存）") && out.contains("没有下一关：已封存"), "{out}");

    let dir = base_of(&home).join("out");
    let (ok, _, err) = cli(&home, &["scans", "render", &sid, dir.to_str().unwrap()]);
    assert!(ok, "{err}");
    let report = fs::read_to_string(dir.join("REPORT.md")).unwrap();
    assert_eq!(fs::read_to_string(dir.join("SUMMARY.md")).unwrap(), "# 摘要\n\n- 证实 1 条\n");
    assert!(report.starts_with("# 安全扫描报告"), "{report}");
    assert!(report.contains("登录页渲染未转义") && report.contains("完整路径") && !report.contains("生成于 None"), "{report}");
    // 内容相同不算冲突；改过才算，--force 覆盖。
    let (ok, _, err) = cli(&home, &["scans", "render", &sid, dir.to_str().unwrap()]);
    assert!(ok, "{err}");
    fs::write(dir.join("REPORT.md"), "x").unwrap();
    let (ok, _, err) = cli(&home, &["scans", "render", &sid, dir.to_str().unwrap()]);
    assert!(!ok && err.contains("E_OUTPUT_EXISTS"), "{err}");
    let (ok, _, err) = cli(&home, &["scans", "render", &sid, dir.to_str().unwrap(), "--force"]);
    assert!(ok, "{err}");
    assert_eq!(fs::read_to_string(dir.join("REPORT.md")).unwrap(), report);

    let (ok, out, err) = cli(&home, &["scans", "log", &sid, "--last", "4"]);
    assert!(ok, "{err}");
    assert!(out.contains("seal"), "{out}");
    let (ok, out, err) = cli(&home, &["scans", "log", &sid, "--tool", "scan_progress"]);
    assert!(ok, "{err}");
    assert!(!out.contains(" scan_record "), "{out}");

    let bundle = base_of(&home).join("bundle.json");
    let (ok, out, err) = cli(&home, &["scans", "export", &sid, bundle.to_str().unwrap()]);
    assert!(ok, "{err}");
    let v: Value = serde_json::from_str(&fs::read_to_string(&bundle).unwrap()).unwrap();
    for k in ["manifest", "state", "progress", "outputs", "templates", "log"] {
        assert!(v.get(k).is_some(), "导出缺 {k}：{out}");
    }
    assert!(v["outputs"].as_array().is_some_and(|a| a.iter().any(|o| o["key"] == "REPORT.md" && o["content"].as_str().is_some_and(|c| c.contains("登录页渲染未转义")))), "{v}");
    // 没封存的扫描没有包可导出。
    let (_, _, err) = cli(&home, &["scans", "export", &s(&again["scan"], "scanId"), bundle.to_str().unwrap()]);
    assert!(err.contains("E_NOT_SEALED"), "{err}");

    let _ = fs::remove_dir_all(base_of(&home));
}

#[test]
fn packs_layers_project_tighten_and_new_domain() {
    let f = fixture();
    let home = &f.home;
    let repo = f.repo.to_str().unwrap();

    // 内嵌三个域；导出 security 作为新域的起点。导出目录不空时拒绝，免得盖掉已有的工作。
    let (ok, out, err) = cli(home, &["packs", "list"]);
    assert!(ok, "{err}");
    for d in ["investigation", "security", "wiki"] {
        assert!(out.contains(d), "{out}");
    }
    let work = base_of(home).join("work");
    let lint_src = work.join("lint");
    let (ok, _, err) = cli(home, &["packs", "export", "security", lint_src.to_str().unwrap()]);
    assert!(ok, "{err}");
    let (_, _, err) = cli(home, &["packs", "export", "security", lint_src.to_str().unwrap()]);
    assert!(err.contains("E_PACK_INVALID"), "{err}");
    let embedded_validation = fs::read_to_string(lint_src.join("templates/fragments/finding.validation.md")).unwrap();
    let loose = "preconditions: { type: text, when: { verdict: validated }, required: true, max: 1000 }";
    assert!(embedded_validation.contains(loose));

    // 项目级收紧 security：validation 的前提字段上限收紧，再换一份方法。
    let d = f.repo.join(".project-scanner/domains/security");
    fs::create_dir_all(d.join("templates/fragments")).unwrap();
    fs::create_dir_all(d.join("method")).unwrap();
    let tightened = embedded_validation.replace(loose, "preconditions: { type: text, when: { verdict: validated }, required: true, max: 10 }");
    fs::write(d.join("templates/fragments/finding.validation.md"), &tightened).unwrap();
    fs::write(d.join("method/discovery.md"), "# 项目的发现方法\n").unwrap();
    let (ok, out, err) = cli(home, &with_root(vec!["packs", "list"], repo));
    assert!(ok && out.contains("embedded+project"), "{err}{out}");
    let (ok, out, err) = cli(home, &with_root(vec!["packs", "show", "security", "--json"], repo));
    assert!(ok, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    let level_of = |p: &str| v["files"].as_array().unwrap().iter().find(|x| x["path"] == p).map(|x| x["level"].as_str().unwrap().to_string()).unwrap_or_default();
    assert_eq!(level_of("templates/fragments/finding.validation.md"), "project");
    assert_eq!(level_of("templates/fragments/finding.discovery.md"), "embedded");
    assert_eq!(level_of("method/discovery.md"), "project");
    let (ok, out, err) = cli(home, &with_root(vec!["packs", "check", "--base", "security", d.to_str().unwrap()], repo));
    assert!(ok, "{err}{out}");

    // 放宽：去掉 required → E_TEMPLATE_LOOSENED；packs list 把它标出来，scan_start 也拒绝。
    fs::write(d.join("templates/fragments/finding.validation.md"), embedded_validation.replace(loose, "preconditions: { type: text, when: { verdict: validated }, max: 1000 }")).unwrap();
    let (ok, _, err) = cli(home, &with_root(vec!["packs", "check", "--base", "security", d.to_str().unwrap()], repo));
    assert!(!ok && err.contains("E_TEMPLATE_LOOSENED"), "{err}");
    let (ok, out, err) = cli(home, &with_root(vec!["packs", "list"], repo));
    assert!(!ok && out.contains("不能放宽"), "{err}{out}");
    let mut m = Mcp::start(home);
    let v = m.call("scan_start", json!({"root": repo, "domain": "security", "prompt": PROMPT}));
    expect_err(&v, "E_TEMPLATE_LOOSENED");

    // 项目级不能改规则：带 pack.json 的覆盖整个拒绝，哪怕片段是对的。
    fs::write(d.join("templates/fragments/finding.validation.md"), &tightened).unwrap();
    fs::copy(lint_src.join("pack.json"), d.join("pack.json")).unwrap();
    let v = m.call("scan_start", json!({"root": repo, "domain": "security", "prompt": PROMPT}));
    expect_err(&v, "E_PACK_INVALID");
    fs::remove_file(d.join("pack.json")).unwrap();

    // 项目级新域：以导出为起点，改 domain / description，packs check 后放进项目根就能用。
    let lint = f.repo.join(".project-scanner/domains/lint");
    copy_dir(&lint_src, &lint);
    let pack_json = fs::read_to_string(lint.join("pack.json")).unwrap().replace("\"domain\": \"security\"", "\"domain\": \"lint\"");
    fs::write(lint.join("pack.json"), &pack_json).unwrap();
    let (ok, out, err) = cli(home, &["packs", "check", lint.to_str().unwrap()]);
    assert!(ok, "{err}{out}");
    // 目录名和 domain 不一致会被 check 指出。
    fs::write(lint_src.join("pack.json"), pack_json.replace("\"domain\": \"lint\"", "\"domain\": \"lint2\"")).unwrap();
    let (ok, out, err) = cli(home, &["packs", "check", lint_src.to_str().unwrap()]);
    assert!(!ok && out.contains("目录名"), "{err}{out}");
    let (ok, out, err) = cli(home, &with_root(vec!["packs", "list"], repo));
    assert!(ok && out.contains("lint") && out.contains("project"), "{err}{out}");
    let v = m.call("scan_start", json!({"root": repo, "domain": "nope", "prompt": PROMPT}));
    expect_err(&v, "E_DOMAIN_UNKNOWN");
    let listed: Vec<String> = v["data"]["domains"].as_array().map(|a| a.iter().map(|x| x["domain"].as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
    assert!(listed.contains(&"lint".to_string()) && listed.contains(&"security".to_string()), "{v}");

    // 用新域起扫描：来源是项目级；security 则是内嵌加项目级叠加，方法取项目级那份，快照也写进扫描目录。
    let v = m.call("scan_start", json!({"root": repo, "domain": "lint", "prompt": PROMPT}));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["pack"]["source"], "project", "{v}");
    assert!(v["pack"]["overlays"].as_array().unwrap().is_empty(), "{v}");
    let v = m.call("scan_start", json!({"root": repo, "domain": "security", "prompt": PROMPT}));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["pack"]["source"], "embedded", "{v}");
    let sid = s(&v, "scanId").to_string();
    let overlays = v["pack"]["overlays"].as_array().unwrap();
    assert_eq!(overlays.len(), 1, "{v}");
    assert_eq!(overlays[0]["level"], "project");
    let files: Vec<&str> = overlays[0]["files"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert!(files.contains(&"method/discovery.md") && files.contains(&"templates/fragments/finding.validation.md"), "{v}");
    // 快照目录：pack.json + 项目级的方法文件。
    let snapshot = find_file(home, "pack").expect("扫描目录里要有 pack/ 快照");
    assert_eq!(fs::read_to_string(snapshot.join("method/discovery.md")).unwrap(), "# 项目的发现方法\n");
    assert!(snapshot.join("templates").exists() == false, "模板集另有自己的快照目录");

    // 当前阶段的方法随上下文来；preflight 没有方法，首次读取进入 planning 后是 planning 的。
    assert!(v["method"].is_null(), "{v}");
    let v = m.call("scan_context", json!({"scanId": sid}));
    assert_eq!(v["phase"], "planning");
    assert_eq!(v["method"]["stage"], "planning");
    assert!(v["method"]["content"].as_str().is_some_and(|c| !c.is_empty()), "{v}");
    let v = m.call("scan_context", json!({"scanId": sid, "view": "pack"}));
    let methods = v["method"].as_array().unwrap();
    assert_eq!(methods.len(), 4, "{v}");
    assert_eq!(methods[1]["stage"], "discovery");
    assert_eq!(methods[1]["content"], "# 项目的发现方法\n");
    assert!(v["rules"]["verdicts"][0] == "validated", "{v}");
    let _ = fs::remove_dir_all(base_of(home));
}

fn copy_dir(from: &Path, to: &Path) {
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            fs::create_dir_all(&dst).unwrap();
            copy_dir(&e.path(), &dst);
        } else {
            fs::copy(e.path(), &dst).unwrap();
        }
    }
}

/// 在数据目录下找名为 name 的目录（扫描目录的布局是内部事，测试不写死）。
fn find_file<'a>(dir: &'a Path, name: &str) -> Option<PathBuf> {
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if e.file_name() == name {
                return Some(p);
            }
            if let Some(found) = find_file(&p, name) {
                return Some(found);
            }
        }
    }
    None
}

fn with_root<'a>(mut args: Vec<&'a str>, repo: &'a str) -> Vec<&'a str> {
    args.push("--root");
    args.push(repo);
    args
}

fn base_of(home: &Path) -> PathBuf {
    home.parent().unwrap().to_path_buf()
}