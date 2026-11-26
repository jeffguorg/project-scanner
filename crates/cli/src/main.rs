//! `project-scanner`：命令行只读和管理；`mcp` 子命令是给 agent 的服务。

mod mcp;

use clap::{Parser, Subcommand};
use project_scanner_kernel as k;
use project_scanner_kernel::store::Home;
use serde_json::Value;
use std::path::PathBuf;
use std::process::exit;

#[derive(Parser)]
#[command(name = "project-scanner", version, about = "项目扫描器：agent 扫描，内核记录、过关卡、渲染报告")]
struct Cli {
    /// 扫描数据目录（默认 ~/.local/share/project-scanner）
    #[arg(long, env = "PSCAN_HOME", global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 启动 MCP stdio 服务（给宿主登记用）
    Mcp,
    /// 自述：版本、数据目录、域包、技能、接入方式
    Info,
    /// 导出内嵌的通用技能（project-scanner，含 references）
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },
    /// 域包：列出、查看、检查、导出（内嵌 / 用户级 / 项目级合并后的）
    Packs {
        #[command(subcommand)]
        cmd: PacksCmd,
    },
    Scans {
        #[command(subcommand)]
        cmd: ScansCmd,
    },
}

#[derive(Subcommand)]
enum SkillsCmd {
    /// 把技能目录写到给定目录（只有一个通用技能；域方法在域包里，用 packs show / export 取）
    Export { dir: PathBuf },
}

#[derive(Subcommand)]
enum PacksCmd {
    /// 列出可用的域：名字、描述、来源、各层
    List {
        /// 项目根（看项目级域包；默认当前目录）
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// 一个域的详情：合并后的规则、每个文件来自哪层、方法文件
    Show {
        domain: String,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// 检查一个域包目录（放进去之前查）。目录名就是域名；--base 时按项目级覆盖的规则查
    Check {
        dir: PathBuf,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// 把合并后的域包写到空目录，作为新域的起点
    Export { domain: String, dir: PathBuf, #[arg(long)] root: Option<PathBuf> },
}

#[derive(Subcommand)]
enum ScansCmd {
    /// 列扫描
    List {
        /// 只列这个目录下的目标
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// 状态：阶段、计数、板块、用户要求、进度、缺口、下一步
    Show {
        id: String,
        #[arg(long)]
        json: bool,
        /// 同时列模板集
        #[arg(long)]
        templates: bool,
        /// 同时输出全部记录
        #[arg(long)]
        records: bool,
        /// 有缺口时退出码 1
        #[arg(long)]
        verify: bool,
    },
    /// 调用日志
    Log {
        id: String,
        #[arg(long)]
        tool: Option<String>,
        #[arg(long)]
        last: Option<usize>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 把封存的渲染结果写到目录（必填）
    Render { id: String, dir: PathBuf, #[arg(long)] force: bool },
    /// 导出封存包为一个 JSON 文件（必填）
    Export { id: String, file: PathBuf },
    /// 删除扫描（连同检查点和日志）
    Delete { id: String, #[arg(long)] yes: bool },
}

fn main() {
    let cli = Cli::parse();
    let home = Home { root: cli.home.unwrap_or_else(k::store::default_home) };
    let code = run(home, cli.cmd);
    exit(code);
}

fn err(e: k::KError) -> i32 {
    eprintln!("{e}");
    2
}

fn run(home: Home, cmd: Cmd) -> i32 {
    match cmd {
        Cmd::Mcp => {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
            match rt.block_on(mcp::run(home)) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("mcp: {e:#}");
                    2
                }
            }
        }
        Cmd::Info => {
            let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            println!("{}", serde_json::to_string_pretty(&k::api::info(&home, &root)).unwrap_or_default());
            0
        }
        Cmd::Skills { cmd } => match cmd {
            SkillsCmd::Export { dir } => match k::pack::export_skills(&dir) {
                Ok(names) => {
                    println!("已导出到 {}：{}（references/ 里有怎么做新域包）", dir.display(), names.join("、"));
                    0
                }
                Err(e) => {
                    eprintln!("{e:#}");
                    2
                }
            },
        },
        Cmd::Packs { cmd } => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match cmd {
                PacksCmd::List { root, json } => {
                    let v = k::pack::describe(&home, root_of(&root, &cwd));
                    if json {
                        println!("{}", serde_json::to_string_pretty(&Value::Array(v.clone())).unwrap_or_default());
                    } else {
                        print_packs(&v);
                    }
                    if v.iter().any(|d| d.get("error").is_some()) { 1 } else { 0 }
                }
                PacksCmd::Show { domain, root, json } => match k::pack::show(&home, root_of(&root, &cwd), &domain) {
                    Ok(v) => {
                        if json {
                            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                        } else {
                            print_pack(&v);
                        }
                        0
                    }
                    Err(e) => err(e),
                },
                PacksCmd::Check { dir, base, root } => {
                    let checked = match base {
                        Some(b) => k::pack::check_overlay(&home, root_of(&root, &cwd), &b, &dir),
                        None => k::pack::check_dir(&dir),
                    };
                    match checked {
                        Ok(c) => {
                            println!("域 {}：{}", c.domain, if c.problems.is_empty() { "可用".to_string() } else { format!("{} 个问题", c.problems.len()) });
                            for p in &c.problems {
                                println!("  - {p}");
                            }
                            if c.problems.is_empty() { 0 } else { 1 }
                        }
                        Err(e) => err(e),
                    }
                }
                PacksCmd::Export { domain, dir, root } => match k::pack::resolve(&home, root_of(&root, &cwd), &domain).and_then(|r| k::pack::export(&dir, &r)) {
                    Ok(written) => {
                        println!("已导出 {} 个文件到 {}（改 domain、description，再 packs check）", written.len(), dir.display());
                        0
                    }
                    Err(e) => err(e),
                },
            }
        }
        Cmd::Scans { cmd } => match cmd {
            ScansCmd::List { root, json } => match k::api::list(&home, root.as_ref().map(|p| p.to_string_lossy().into_owned()).as_deref(), true) {
                Ok(v) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    } else {
                        print_list(&v);
                    }
                    0
                }
                Err(e) => err(e),
            },
            ScansCmd::Show { id, json, templates, records, verify } => match k::api::summary(&home, &id) {
                Ok(v) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    } else {
                        print_summary(&v);
                    }
                    if templates {
                        match k::api::templates_of(&home, &id) {
                            Ok(t) => println!("{}", serde_json::to_string_pretty(&t).unwrap_or_default()),
                            Err(e) => return err(e),
                        }
                    }
                    if records {
                        match k::api::records(&home, &id) {
                            Ok(r) => println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default()),
                            Err(e) => return err(e),
                        }
                    }
                    let gaps = v["gaps"].as_array().map(|a| a.len()).unwrap_or(0);
                    if verify && gaps > 0 {
                        1
                    } else {
                        0
                    }
                }
                Err(e) => err(e),
            },
            ScansCmd::Log { id, tool, last, since, json } => {
                let filter = k::log::LogFilter { tool, last, since };
                match k::api::log_entries(&home, &id, &filter) {
                    Ok(v) => {
                        if json {
                            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                        } else {
                            print_log(&v);
                        }
                        0
                    }
                    Err(e) => err(e),
                }
            }
            ScansCmd::Render { id, dir, force } => match k::api::render_out(&home, &id, &dir, force) {
                Ok(v) => {
                    for w in v["written"].as_array().cloned().unwrap_or_default() {
                        println!("{}", dir.join(w.as_str().unwrap_or("")).display());
                    }
                    0
                }
                Err(e) => err(e),
            },
            ScansCmd::Export { id, file } => match k::api::export(&home, &id, &file) {
                Ok(_) => {
                    println!("{}", file.display());
                    0
                }
                Err(e) => err(e),
            },
            ScansCmd::Delete { id, yes } => match k::api::delete(&home, &id, yes) {
                Ok(_) => {
                    println!("已删除 {id}");
                    0
                }
                Err(e) => err(e),
            },
        },
    }
}

/// --root 没给就是当前目录。
fn root_of<'a>(r: &'a Option<PathBuf>, cwd: &'a PathBuf) -> &'a std::path::Path {
    r.as_ref().map(|p| p.as_path()).unwrap_or(cwd.as_path())
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn print_list(v: &Value) {
    let items = v["scans"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        println!("没有扫描");
        return;
    }
    println!("{:<14} {:<14} {:<18} {:<6} {:<25} {}", "标识", "域", "阶段", "封存", "更新时间", "要求");
    for it in items {
        println!(
            "{:<14} {:<14} {:<18} {:<6} {:<25} {}",
            s(&it, "scanId"),
            s(&it, "domain"),
            s(&it, "phase"),
            if it["sealed"].as_bool().unwrap_or(false) { "是" } else { "否" },
            s(&it, "updatedAt"),
            s(&it, "prompt")
        );
    }
}

fn print_summary(v: &Value) {
    let scan = &v["scan"];
    println!(
        "扫描 {}  域 {}  阶段 {}{}",
        s(scan, "scanId"),
        s(scan, "domain"),
        s(scan, "phase"),
        if scan["sealed"].as_bool().unwrap_or(false) { "（已封存）" } else { "" }
    );
    println!("目标 {}  修订 {}{}", s(scan, "root"), s(scan, "revision"), if scan["dirty"].as_bool().unwrap_or(false) { "（含未提交改动）" } else { "" });
    let w = &scan["writer"];
    println!("写者 {}（pid {}，自 {}）  更新 {}", s(w, "sessionId"), w["pid"], s(w, "since"), s(scan, "updatedAt"));
    println!("模板集 {}{}", s(&v["templates"], "digest"), if v["templates"]["frozen"].as_bool().unwrap_or(false) { "（已冻结）" } else { "" });
    println!();
    println!("用户要求：");
    for r in v["userRequests"].as_array().cloned().unwrap_or_default() {
        let text = s(&r, "text").replace('\n', "\n    ");
        println!("  - {}    （{}，{}）", text, s(&r, "at"), s(&r, "source"));
    }
    println!();
    let p = &v["progress"];
    let active: Vec<&str> = p["activeSections"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    println!("进度：正在处理 {}{}", if active.is_empty() { "—".to_string() } else { active.join("、") }, p["note"].as_str().map(|n| format!("；{n}")).unwrap_or_default());
    if let Some(q) = p["waitingForDecision"].as_str() {
        println!("等待决定：{q}");
    }
    let reopened: Vec<&str> = p["reopenedSections"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    if !reopened.is_empty() {
        println!("重开的板块：{}", reopened.join("、"));
    }
    println!();
    let c = &v["counts"];
    let inv = &c["inventory"];
    println!(
        "计数：板块 {}  断言 {}  带警告 {}  路径 {}/{}  范围：文件 {}，已覆盖 {}，已排除 {}，未覆盖 {}",
        fmt_map(&c["sections"]),
        fmt_map(&c["findings"]),
        c["withWarnings"],
        c["paths"]["complete"],
        c["paths"]["required"],
        inv["files"],
        inv["covered"],
        inv["excluded"],
        inv["uncovered"]
    );
    println!();
    println!("板块：");
    println!("  {:<6} {:<24} {:<16} {:<14} {:<6} {}", "标识", "标签", "类别", "状态", "断言", "处置");
    for sec in v["sections"].as_array().cloned().unwrap_or_default() {
        println!(
            "  {:<6} {:<24} {:<16} {:<14} {:<6} {}",
            s(&sec, "id"),
            s(&sec, "label"),
            s(&sec, "kind"),
            format!("{}{}", s(&sec, "status"), if sec["reopened"].as_bool().unwrap_or(false) { "（重开）" } else { "" }),
            fmt_map(&sec["findingCounts"]),
            sec["disposition"].as_str().map(|d| format!("{d}：{}", s(&sec, "dispositionReason"))).unwrap_or_default()
        );
    }
    println!();
    let gaps = v["gaps"].as_array().cloned().unwrap_or_default();
    match v["nextGate"].as_str() {
        Some(gate) if gaps.is_empty() => println!("下一关（{gate}）：没有缺口"),
        Some(gate) => {
            println!("下一关（{gate}）的缺口：");
            for g in gaps {
                println!("  - [{}] {}", s(&g, "code"), s(&g, "message"));
            }
        }
        None => println!("没有下一关：已封存"),
    }
    println!("下一步：{}", s(&v, "nextStep"));
}

fn fmt_map(v: &Value) -> String {
    match v.as_object() {
        Some(m) if !m.is_empty() => m.iter().map(|(k, x)| format!("{k} {x}")).collect::<Vec<_>>().join(" "),
        _ => "—".to_string(),
    }
}

fn print_log(v: &Value) {
    for e in v["entries"].as_array().cloned().unwrap_or_default() {
        let kind = s(&e, "kind");
        let head = format!("{:>5} {} {}", e["seq"], s(&e, "at"), kind);
        let body = if kind == "call" {
            format!(
                "{} {} {} {}{}",
                s(&e, "writer"),
                s(&e, "tool"),
                s(&e, "result"),
                e["message"].as_str().map(|m| format!(" {m}")).unwrap_or_default(),
                e["ms"].as_u64().map(|ms| format!(" ({ms}ms)")).unwrap_or_default()
            )
        } else {
            format!("{} {} {}", s(&e, "event"), s(&e, "source"), s(&e, "message"))
        };
        println!("{head}  {body}");
    }
}

fn print_packs(v: &[Value]) {
    if v.is_empty() {
        println!("没有可用的域包");
        return;
    }
    println!("{:<16} {:<10} {:<24} {}", "域", "来源", "层", "描述");
    for d in v {
        let levels: Vec<&str> = d["levels"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
        println!(
            "{:<16} {:<10} {:<24} {}",
            s(d, "domain"),
            s(d, "source"),
            levels.join("+"),
            d["description"].as_str().unwrap_or_else(|| d["error"]["message"].as_str().unwrap_or(""))
        );
    }
}

fn print_pack(v: &Value) {
    println!("域 {}（{}）  规则来自 {}  摘要 {}", s(v, "domain"), s(v, "description"), s(v, "source"), s(v, "digest"));
    println!("方法：");
    for m in v["method"].as_array().cloned().unwrap_or_default() {
        println!("  {:<18} {}", s(&m, "stage"), s(&m, "path"));
    }
    let refs: Vec<&str> = v["references"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    if !refs.is_empty() {
        println!("参考：{}", refs.join("、"));
    }
    println!("文件：");
    for f in v["files"].as_array().cloned().unwrap_or_default() {
        println!("  {:<10} {}", s(&f, "level"), s(&f, "path"));
    }
}
