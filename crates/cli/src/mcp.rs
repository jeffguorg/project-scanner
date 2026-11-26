//! MCP stdio 服务：五个工具，薄壳。所有逻辑在内核；这里只做输入 schema、返回形状、日志由内核记。

use project_scanner_kernel as k;
use project_scanner_kernel::api::Session;
use project_scanner_kernel::store::Home;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, model::*, tool, tool_handler, tool_router, transport::stdio};
use serde::Deserialize;
use serde_json::{json, Value};

pub const INSTRUCTIONS: &str = "\
project-scanner 给 agent 四样东西：任务契约、方法论、状态与检查点、一份必须满足才能结束的放行条件。
扫描器不扫描，agent 才是扫描器。

流程：scan_start 新建（或 resume 接管）→ scan_context 读上下文（首次读取自动进入 planning）→ scan_record 记录
→ 关卡通过自动推进，或 scan_progress 声明推进 → 声明 finalized 后封存并渲染。

写入的三种后果：结构性错误（E_*）不写入，要改；记录警告（W_*）写入并随记录返回，进下一阶段前必须清除；
关卡拒绝（G_* 缺口）内容保留、阶段不前进。gaps 和 guidance 原样转述即可，人用 scans show 看到的是同一段。

定位格式：<commit>[+dirty]:<relpath>:block_start(<line_text>)[:block_end(<line_text>)][:+<offset>]
语义锚（identity.anchor）：小写字母数字、连字符分段，描述失效之处的作用；增量扫描靠它认出同一条。

域：domain 不只这三个，可用域在下面列出（packs list 同样）；每个阶段的方法随 scan_context 的 method 字段给出，
照着做。都不合适可以自己做域包：project-scanner packs export <域> <目录> 导出一份改，packs check 查，放到
PSCAN_HOME/packs/<域名>/（自己的机器）或 <项目根>/.project-scanner/domains/<域名>/（随仓库走，叠在已有域上时只能收紧）。
技能：project-scanner skills export <目录> 导出通用技能 project-scanner（含 references/domain-pack.md）。";

/// 服务说明：固定的话 + 当前可用的域（按服务启动时的根算项目级）。
fn instructions(home: &Home) -> String {
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let domains: Vec<String> = k::pack::describe(home, &root)
        .iter()
        .map(|d| match d["description"].as_str() {
            Some(desc) => format!("{}（{desc}；{}）", d["domain"].as_str().unwrap_or(""), d["source"].as_str().unwrap_or("")),
            None => format!("{}（域包有问题，packs list 看）", d["domain"].as_str().unwrap_or("")),
        })
        .collect();
    format!("{INSTRUCTIONS}\n\n可用的域：{}", domains.join("、"))
}

// ─── 输入 ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, serde::Serialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanStartInput {
    /// 恢复并接管未封存的扫描：给扫描标识。给了它就忽略其他字段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    /// 目标目录，默认当前目录；必须是 git 仓库
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// 域：security | wiki | investigation，必填
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// 增量的基准扫描标识或 latest（这一版未实现）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incremental_from: Option<String>,
    /// 用户的要求，原话，新扫描必填
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanContextInput {
    /// 扫描标识
    pub scan_id: String,
    /// summary（默认，概要）| fragments（各阶段字段表）| templates（完整模板集）| records（分页读记录）| inventory（范围与覆盖）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// records：按板块筛选
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
    /// records：按状态筛选
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// records：按断言类别筛选
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// records：finding（默认）或 section
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// records / inventory：直接按标识取
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    /// records / inventory：分页游标
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u64>,
    /// records / inventory：每页条数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// inventory：按路径前缀筛
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// inventory：covered | excluded | uncovered
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateFileIn {
    /// 相对模板集根的路径：fragments/…、outputs/…、partials/…
    pub path: String,
    /// 文件全文：'---' 前置元数据 + 正文
    pub content: String,
}

#[derive(Debug, Deserialize, serde::Serialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanTemplatesInput {
    pub scan_id: String,
    /// 追加新文件
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<TemplateFileIn>,
    /// 收紧已有片段（或追加）：只能加必填、收窄取值、调小上限
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tighten: Vec<TemplateFileIn>,
}

#[derive(Debug, Deserialize, serde::Serialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanRecordInput {
    pub scan_id: String,
    /// 操作列表，每项 { op: "...", ... }。op 取值：
    /// section.create{label,kind,paths,questions} section.update{id,label?,paths?,questions?} section.merge{id,into} section.abolish{id,reason}
    /// finding.create{sectionId,kind,identity{anchor,instance?},anchors,fields} finding.update{id,anchors?,fields} finding.retract{id,reason}
    /// disposition.set{sectionId,disposition,reason} verdict.attach{id,fields} evidence.append{id,anchors}
    /// path.attach{id,steps[{role,anchor|gap,note?}],prerequisites?} path.update{id,index,steps,prerequisites?} path.retract{id,index,reason}
    /// exclusion.add{path,reason} exclusion.remove{path}
    pub ops: Vec<Value>,
}

#[derive(Debug, Deserialize, serde::Serialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Waiting {
    /// 要人决定的问题，写清楚
    pub question: String,
}

#[derive(Debug, Deserialize, serde::Serialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanProgressInput {
    pub scan_id: String,
    /// 正在处理的板块
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_sections: Option<Vec<String>>,
    /// 活动说明，原样展示
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 追加的用户要求，原话
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_request: Option<String>,
    /// { question } 标记等待决定；null 清除
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Option<Waiting>>")]
    pub waiting: Option<Option<Waiting>>,
    /// 重开的板块：允许再新建断言
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reopen: Vec<String>,
    /// 声明进入下一阶段（只能逐级）；finalized 触发封存
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advance: Option<String>,
}

fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    let v = Option::<T>::deserialize(de)?;
    Ok(Some(v))
}

fn input_value<T: serde::Serialize>(t: &T) -> Value {
    serde_json::to_value(t).unwrap_or(Value::Null)
}

// ─── 服务 ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Scanner {
    #[allow(dead_code, reason = "tool_handler 宏通过这个字段找路由")]
    tool_router: ToolRouter<Self>,
    home: Home,
    session: Session,
}

/// 设计 §8：五个工具返回统一形状 `{ ok, scanId, phase, written, … }`。内核的读类返回是概要视图，
/// 这里补齐顶层的 ok、scanId、phase、written（已有的不动）；错误同样带 ok 和 scanId。
fn to_result(r: k::KResult<Value>, scan_id: Option<&str>, written: bool) -> Result<CallToolResult, McpError> {
    match r {
        Ok(mut v) => {
            if let Value::Object(m) = &mut v {
                let scan = m.get("scan").cloned();
                for (k, val) in [("ok", json!(true)), ("written", json!(written))] {
                    m.entry(k.to_string()).or_insert(val);
                }
                if let Some(scan) = scan {
                    m.entry("scanId".to_string()).or_insert(scan["scanId"].clone());
                    m.entry("phase".to_string()).or_insert(scan["phase"].clone());
                }
            }
            Ok(CallToolResult::structured(v))
        }
        Err(e) => Ok(CallToolResult::structured_error(json!({
            "ok": false, "scanId": scan_id, "code": e.code, "message": e.message, "data": e.data,
        }))),
    }
}

#[tool_router]
impl Scanner {
    pub fn new(home: Home) -> Self {
        Self { tool_router: Self::tool_router(), home, session: Session::new() }
    }

    #[tool(description = "新建扫描，或按标识接管未封存的扫描。新建：{ root?, domain, prompt }——root 默认当前目录，必须是 git 仓库；domain 是 security | wiki | investigation；prompt 是用户的要求原话。内核做 preflight：固定修订和内容摘要、清点范围内的文本文件、读策略文件、复制域的默认模板集。接管：{ resume: 扫描标识 }，之后旧会话的写入会被拒绝。返回概要视图：阶段、缺口、下一步、字段表、用户要求、进度、计数。读取不改状态。")]
    async fn scan_start(&self, Parameters(req): Parameters<ScanStartInput>) -> Result<CallToolResult, McpError> {
        let input = input_value(&req);
        let r = k::api::start(&self.home, &self.session, &input);
        // 新建或接管都改了扫描数据；封存后的 resume 只读。
        let written = r.as_ref().ok().is_some_and(|v| v["sealed"] != true);
        to_result(r, input["resume"].as_str(), written)
    }

    #[tool(description = "读上下文，不改状态（例外：写者首次读取时从 preflight 进入 planning）。默认 summary：阶段、下一关的缺口、本阶段指引、当前阶段各记录类型的字段表、用户要求、进度、计数、板块状态、策略。view=fragments 看全部阶段的字段表；templates 看完整模板集和摘要；records 分页读板块/断言（可按 sectionId、status、kind、ids 筛）；inventory 看目录级覆盖和文件清单（mark=uncovered 找没归属的文件）。被压缩后先读 summary，再按需读记录。")]
    async fn scan_context(&self, Parameters(req): Parameters<ScanContextInput>) -> Result<CallToolResult, McpError> {
        let input = input_value(&req);
        to_result(k::api::context(&self.home, &self.session, &input), Some(req.scan_id.as_str()), false)
    }

    #[tool(description = "追加或收紧模板集：add 新文件，tighten 已有片段。文件 = YAML 前置元数据 + Jinja 正文。片段声明字段：type(text|enum|anchors|refs|list|count)、values、required、when、min、max。只能追加和收紧：删字段、放宽必填或枚举、调大上限 → E_TEMPLATE_LOOSENED。validation 之后模板集冻结：只能追加输出、收紧字段。登记后重算全部记录的警告。")]
    async fn scan_templates(&self, Parameters(req): Parameters<ScanTemplatesInput>) -> Result<CallToolResult, McpError> {
        let input = input_value(&req);
        to_result(k::api::templates_write(&self.home, &self.session, &input), Some(req.scan_id.as_str()), true)
    }

    #[tool(description = "记录、更新、废除板块、断言和断言的补充记录。一批操作是一个检查点：先全部做结构校验（写者、阶段权限、定位、身份），任一结构性错误则整批不写入；通过后按模板片段校验，缺必填、类型不符、定位过期作为警告随记录保存并返回，带警告的记录过不了下一关。断言只能在 discovery 或重开的板块新建；validation 之后只能附结论、证据、路径。重开的板块被写过一次后恢复普通状态。同一内容重复提交不产生新检查点。返回 ids、warnings、gaps、是否自动推进。")]
    async fn scan_record(&self, Parameters(req): Parameters<ScanRecordInput>) -> Result<CallToolResult, McpError> {
        let input = input_value(&req);
        to_result(k::api::record(&self.home, &self.session, &input), Some(req.scan_id.as_str()), true)
    }

    #[tool(description = "同步进展：activeSections 正在处理的板块；note 活动说明（原样展示）；appendRequest 追加用户要求（原话）；waiting { question } 标记等待决定、null 清除（标记期间界面不催）；reopen 把板块退回调查，之后可在其中新建断言；advance 声明进入下一阶段（只能逐级），过的是和自动推进同一个关卡，不通过返回缺口且阶段不变；声明 finalized 跑全部关卡并渲染，通过即封存。")]
    async fn scan_progress(&self, Parameters(req): Parameters<ScanProgressInput>) -> Result<CallToolResult, McpError> {
        let input = input_value(&req);
        to_result(k::api::progress(&self.home, &self.session, &input), Some(req.scan_id.as_str()), true)
    }
}

#[tool_handler]
impl ServerHandler for Scanner {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("project-scanner", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions(&self.home))
    }
}

pub async fn run(home: Home) -> anyhow::Result<()> {
    let scanner = Scanner::new(home);
    let service = scanner.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
