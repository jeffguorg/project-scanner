//! 域包：`pack.json` + `templates/` + `method/`。
//!
//! 三层来源按文件合并：内嵌（编译期打包）、用户级（`PSCAN_HOME/packs/<domain>/`）、项目级
//! （`<扫描根>/.project-scanner/domains/<domain>/`），项目级最优先。用户级是自己机器上的文件，可以
//! 替换任何文件；项目级随仓库来，叠在已有域上时只能追加和收紧——扫别人的仓库时它是外部内容，
//! 放宽会让一份"已封存"的报告比看上去的要求松。
//!
//! 解析时就把整套检查做完（规则、模板、必需的片段、试渲染），有问题的域包用不了，错误里说清改哪。
//! 扫描开始时把生效的域包固定成快照，之后的操作只读快照。

use crate::error::{KError, KResult};
use crate::model::{Overlay, PackMeta, Phase};
use crate::store::Home;
use crate::templates::{self, TemplateSet};
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

static DOMAINS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../domains");
static SKILL: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../skills/project-scanner");

/// 项目级域包的目录，相对扫描根。
pub const PROJECT_DIR: &str = ".project-scanner";
/// 可以给方法的阶段。
pub const METHOD_STAGES: [&str; 4] = ["planning", "discovery", "validation", "analyze_full_path"];
/// 缺了这些通用片段，那类记录就永远带着 W_NO_FRAGMENT 过不了关。finding 在 analyze_full_path 的片段是可选的
/// （security 有，wiki / investigation 没有：那个阶段由路径片段负责）。
const REQUIRED_FRAGMENTS: [(&str, Phase); 5] = [
    ("section", Phase::Planning),
    ("exclusion", Phase::Planning),
    ("finding", Phase::Discovery),
    ("finding", Phase::Validation),
    ("path", Phase::AnalyzeFullPath),
];

// ─── 规则 ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Pack {
    pub domain: String,
    /// 一句话：这个域查什么。给人和 agent 选域用。
    pub description: String,
    pub section_kinds: Vec<String>,
    pub finding_kinds: Vec<String>,
    pub path_required_kinds: Vec<String>,
    pub path_roles: Vec<String>,
    pub origin_roles: Vec<String>,
    pub outcome_roles: Vec<String>,
    pub verdicts: Vec<String>,
    pub severities: Vec<String>,
    pub confidences: Vec<String>,
    pub dispositions: Vec<String>,
    pub default_exclusions: Vec<String>,
    pub policy_file: Option<String>,
    pub anchor_naming: String,
    /// 阶段 → 方法文件（相对域包根）。
    pub method: BTreeMap<String, String>,
    /// 按需读的参考文件（相对域包根）。
    pub references: Vec<String>,
    /// 域包里全部方法与参考文件的正文（相对路径 → 内容）。不写进 pack.json，来自解析或快照。
    #[serde(skip)]
    pub docs: BTreeMap<String, String>,
}

impl Pack {
    /// 某个阶段的方法正文。preflight 和 finalized 没有方法。
    pub fn method_for(&self, phase: Phase) -> Option<&str> {
        let stage = match phase {
            Phase::Planning => "planning",
            Phase::Discovery => "discovery",
            Phase::Validation => "validation",
            Phase::AnalyzeFullPath => "analyze_full_path",
            _ => return None,
        };
        self.method.get(stage).and_then(|p| self.docs.get(p)).map(|s| s.as_str())
    }

    /// 规则本身（不含方法正文），给 JSON 用。
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// 合并后的域包。
#[derive(Debug, Clone)]
pub struct Resolved {
    pub pack: Pack,
    /// 生效的全部文件（相对域包根）：`pack.json`、`templates/**`、`method/**`。
    pub files: BTreeMap<String, String>,
    pub meta: PackMeta,
}

impl Resolved {
    /// 模板集的文件（路径相对 `templates/`）。
    pub fn template_files(&self) -> Vec<(String, String)> {
        templates_of(&self.files)
    }
}

// ─── 三层来源 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Embedded,
    User,
    Project,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Embedded, Level::User, Level::Project];

    pub fn name(self) -> &'static str {
        match self {
            Level::Embedded => "embedded",
            Level::User => "user",
            Level::Project => "project",
        }
    }

    fn dir(self, home: &Home, root: &Path, domain: &str) -> Option<PathBuf> {
        match self {
            Level::Embedded => None,
            Level::User => Some(home.packs_dir().join(domain)),
            Level::Project => Some(project_domains_dir(root).join(domain)),
        }
    }
}

/// 项目级域包的根：`<扫描根>/.project-scanner/domains/`。
pub fn project_domains_dir(root: &Path) -> PathBuf {
    root.join(PROJECT_DIR).join("domains")
}

pub fn valid_domain_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

// 子目录里的 entry 路径仍然是相对根的，所以查找一律从根出发。
fn name_of(d: &Dir<'_>) -> String {
    d.path().file_name().unwrap().to_string_lossy().into_owned()
}

/// 域包里认的文件：其余的读了也不管。
fn known(path: &str) -> bool {
    path == "pack.json" || path.starts_with("templates/") || path.starts_with("method/")
}

fn walk_embedded(dir: &Dir<'_>, prefix: &str, out: &mut Vec<(String, String)>) {
    for f in dir.files() {
        let rel = format!("{prefix}{}", f.path().file_name().unwrap().to_string_lossy());
        if let Some(s) = f.contents_utf8() {
            out.push((rel, s.to_string()));
        }
    }
    for d in dir.dirs() {
        let rel = format!("{prefix}{}/", d.path().file_name().unwrap().to_string_lossy());
        walk_embedded(d, &rel, out);
    }
}

fn walk_fs(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<(String, PathBuf)> = rd.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect();
    entries.sort();
    for (name, p) in entries {
        let rel = format!("{prefix}{name}");
        if p.is_dir() {
            walk_fs(&p, &format!("{rel}/"), out);
        } else if let Ok(s) = fs::read_to_string(&p) {
            out.push((rel, s));
        }
    }
}

/// 某一层某个域的全部文件，只保留认得的。
fn level_files(level: Level, home: &Home, root: &Path, domain: &str) -> BTreeMap<String, String> {
    let mut out = Vec::new();
    match level {
        Level::Embedded => {
            if let Some(d) = DOMAINS.get_dir(domain) {
                walk_embedded(d, "", &mut out);
            }
        }
        _ => {
            if let Some(dir) = level.dir(home, root, domain) {
                walk_fs(&dir, "", &mut out);
            }
        }
    }
    out.into_iter().filter(|(p, _)| known(p)).collect()
}

/// 一个目录里的域包文件（放进去之前验证用）。
fn read_dir_files(dir: &Path) -> BTreeMap<String, String> {
    let mut out = Vec::new();
    walk_fs(dir, "", &mut out);
    out.into_iter().filter(|(p, _)| known(p)).collect()
}

fn templates_of(files: &BTreeMap<String, String>) -> Vec<(String, String)> {
    files
        .iter()
        .filter(|(p, _)| p.starts_with("templates/"))
        .map(|(p, c)| (p.trim_start_matches("templates/").to_string(), c.clone()))
        .collect()
}

/// 各层出现的域（按名排序）。只看带 `pack.json` 的目录。
pub fn domain_names(home: &Home, root: &Path) -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for d in DOMAINS.dirs() {
        if d.get_file(format!("{}/pack.json", name_of(d))).is_some() {
            names.insert(name_of(d));
        }
    }
    for dir in [home.packs_dir(), project_domains_dir(root)] {
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.path().join("pack.json").is_file() {
                    names.insert(e.file_name().to_string_lossy().into_owned());
                }
            }
        }
    }
    names.into_iter().collect()
}

fn levels_of(home: &Home, root: &Path, domain: &str) -> Vec<&'static str> {
    Level::ALL.iter().filter(|l| !level_files(**l, home, root, domain).is_empty()).map(|l| l.name()).collect()
}

// ─── 解析与合并 ──────────────────────────────────────────────────────────────

/// 三层合并后的域包。域名不合法或哪儿都没有 → `E_DOMAIN_UNKNOWN`（列出可用的域）；
/// 规则、模板、片段、试渲染有任何问题 → `E_PACK_INVALID`，问题逐条列在消息里。
pub fn resolve(home: &Home, root: &Path, domain: &str) -> KResult<Resolved> {
    if !valid_domain_name(domain) {
        return Err(unknown_domain(home, root, domain));
    }
    let per_level: Vec<BTreeMap<String, String>> = Level::ALL.iter().map(|l| level_files(*l, home, root, domain)).collect();
    if per_level.iter().all(BTreeMap::is_empty) {
        return Err(unknown_domain(home, root, domain));
    }
    // 规则来自哪一层：用户级优先于内嵌；项目级只有在下面都没有规则时才是新域。
    let has_rules = |i: usize| per_level[i].contains_key("pack.json");
    let source = if has_rules(1) { 1 } else if has_rules(0) { 0 } else { 2 };
    if source != 2 && has_rules(2) {
        return Err(KError::new(
            "E_PACK_INVALID",
            format!("项目级不能改规则：{domain} 已有{}的 pack.json。要改规则就新起一个域名", if source == 0 { "内嵌" } else { "用户级" }),
        ));
    }
    let mut files = per_level[0].clone();
    let mut overlays: Vec<Overlay> = Vec::new();
    // 用户级是自己机器上的文件：整份替换或追加都行。
    if !per_level[1].is_empty() {
        files.extend(per_level[1].iter().map(|(k, v)| (k.clone(), v.clone())));
        overlays.push(Overlay { level: Level::User.name().into(), files: per_level[1].keys().cloned().collect() });
    }
    if !per_level[2].is_empty() {
        if source == 2 {
            files.extend(per_level[2].iter().map(|(k, v)| (k.clone(), v.clone())));
        } else {
            merge_restricted(&mut files, &per_level[2])?;
            overlays.push(Overlay { level: Level::Project.name().into(), files: per_level[2].keys().cloned().collect() });
        }
    }
    finish(files, domain, Level::ALL[source].name(), overlays)
}

/// 把一组文件当作项目级覆盖叠到已有的文件上：只能收紧片段、追加导航页输出、改方法。
fn merge_restricted(base: &mut BTreeMap<String, String>, over: &BTreeMap<String, String>) -> KResult<()> {
    for (path, content) in over {
        match base.get(path) {
            None => {
                if path.starts_with("templates/outputs/") {
                    let tf = templates::parse_file(path.trim_start_matches("templates/"), content)?;
                    if !matches!(&tf.meta, templates::Meta::Output { select, .. } if select == "sections") {
                        return Err(KError::new(
                            "E_PACK_INVALID",
                            format!("项目级只能追加导航页输出（select: sections）：{path}。已有的输出已经接纳记录，再加一份，记录就会被接纳两次"),
                        ));
                    }
                }
                base.insert(path.clone(), content.clone());
            }
            Some(old) => {
                if path.starts_with("templates/fragments/") {
                    let rel = path.trim_start_matches("templates/").to_string();
                    let o = templates::parse_file(&rel, old)?;
                    let n = templates::parse_file(&rel, content)?;
                    if let Some(why) = templates::loosen(&o, &n) {
                        return Err(KError::new("E_TEMPLATE_LOOSENED", format!("项目级只能收紧，不能放宽或删除：{path} {why}")));
                    }
                    base.insert(path.clone(), content.clone());
                } else if path.starts_with("method/") {
                    base.insert(path.clone(), content.clone());
                } else {
                    return Err(KError::new(
                        "E_PACK_INVALID",
                        format!("项目级不能替换已有的文件：{path}。能做的是收紧片段、追加导航页输出、改方法；要换输出或 partial 就新起一个域名"),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn assemble(files: &BTreeMap<String, String>, domain: &str) -> KResult<Pack> {
    let text = files.get("pack.json").ok_or_else(|| KError::new("E_DOMAIN_BROKEN", format!("域 {domain} 没有 pack.json")))?;
    let mut pack: Pack =
        serde_json::from_str(text).map_err(|e| KError::new("E_DOMAIN_BROKEN", format!("域 {domain} 的 pack.json 解析失败：{e}")))?;
    if pack.domain.is_empty() {
        pack.domain = domain.to_string();
    }
    for (p, c) in files {
        if p.starts_with("method/") {
            pack.docs.insert(p.clone(), c.clone());
        }
    }
    Ok(pack)
}

fn digest_of(files: &BTreeMap<String, String>) -> String {
    let mut input = String::new();
    for (p, c) in files {
        if p == "pack.json" || p.starts_with("method/") {
            input.push_str(p);
            input.push('\0');
            input.push_str(c);
            input.push('\n');
        }
    }
    crate::sha256_hex(input.as_bytes())
}

fn finish(files: BTreeMap<String, String>, domain: &str, source: &str, overlays: Vec<Overlay>) -> KResult<Resolved> {
    let pack = assemble(&files, domain)?;
    let problems = check(&pack, &files, domain);
    if !problems.is_empty() {
        let list = problems.iter().map(|p| format!("  - {p}")).collect::<Vec<_>>().join("\n");
        return Err(KError::new("E_PACK_INVALID", format!("域包 {domain} 不可用：\n{list}")).with_data(json!({ "domain": domain, "problems": problems })));
    }
    let meta = PackMeta { domain: domain.into(), description: pack.description.clone(), digest: digest_of(&files), source: source.into(), overlays };
    Ok(Resolved { pack, files, meta })
}

/// 一套完整检查，返回问题列表（空表示可用）。`resolve` 和 `packs check` 共用。
pub fn check(pack: &Pack, files: &BTreeMap<String, String>, domain: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if pack.domain != domain {
        problems.push(format!("pack.json 的 domain 是 {:?}，和目录名 {domain:?} 不一致", pack.domain));
    }
    if pack.description.trim().is_empty() {
        problems.push("pack.json 缺 description：一句话说明这个域查什么".into());
    }
    for (name, list) in [
        ("sectionKinds", &pack.section_kinds),
        ("findingKinds", &pack.finding_kinds),
        ("pathRoles", &pack.path_roles),
        ("verdicts", &pack.verdicts),
        ("dispositions", &pack.dispositions),
    ] {
        if list.is_empty() {
            problems.push(format!("pack.json 的 {name} 不能为空"));
        }
    }
    for (name, sub, sup) in [
        ("pathRequiredKinds", &pack.path_required_kinds, &pack.finding_kinds),
        ("originRoles", &pack.origin_roles, &pack.path_roles),
        ("outcomeRoles", &pack.outcome_roles, &pack.path_roles),
    ] {
        let stray: Vec<String> = sub.iter().filter(|x| !sup.contains(x)).map(|x| x.to_string()).collect();
        if !stray.is_empty() {
            problems.push(format!("pack.json 的 {name} 里有不在 {} 中的取值：{}", if name == "pathRequiredKinds" { "findingKinds" } else { "pathRoles" }, stray.join("、")));
        }
    }
    if pack.anchor_naming.trim().is_empty() {
        problems.push("pack.json 缺 anchorNaming：语义锚的命名约定，原样给 agent".into());
    }
    if let Some(p) = &pack.policy_file {
        if p.starts_with('/') || p.split('/').any(|s| s == "..") {
            problems.push(format!("policyFile {p:?} 要是相对项目根的路径"));
        }
    }
    for (stage, path) in &pack.method {
        if !METHOD_STAGES.contains(&stage.as_str()) {
            problems.push(format!("method 的阶段 {stage:?} 不认识，只能是 {}", METHOD_STAGES.join("、")));
        }
        if !files.contains_key(path) {
            problems.push(format!("method 指向的文件不存在：{path}"));
        }
    }
    for path in &pack.references {
        if !files.contains_key(path) {
            problems.push(format!("references 指向的文件不存在：{path}"));
        }
    }

    let set = match TemplateSet::build(templates_of(files)) {
        Ok(s) => s,
        Err(e) => {
            problems.push(format!("模板集：{} {}", e.code, e.message));
            return problems;
        }
    };
    for (record, stage) in REQUIRED_FRAGMENTS {
        if set.fragment_for(record, stage, None).is_none() {
            problems.push(format!("缺通用片段：{record} 在 {} 阶段（select 不带 kind 的那一份）", stage.as_str()));
        }
    }
    let (all, section) = crate::gates::output_counts(&set);
    if all + section != 1 {
        problems.push(format!("接纳记录的输出要恰好一份（select: all 或 select: section），现在 all {all} 份、section {section} 份"));
    }
    if let Some(f) = set.fragment_for("finding", Phase::Validation, None) {
        if let templates::Meta::Fragment { fields, .. } = &f.meta {
            if let Some(d) = fields.get("verdict") {
                if d.ty == "enum" {
                    let declared: BTreeSet<&str> = d.values.as_deref().unwrap_or(&[]).iter().map(|s| s.as_str()).collect();
                    let rules: BTreeSet<&str> = pack.verdicts.iter().map(|s| s.as_str()).collect();
                    if declared != rules {
                        problems.push(format!(
                            "pack.json 的 verdicts（{}）要和 validation 片段里 verdict 字段的枚举（{}）一致",
                            pack.verdicts.join("、"),
                            d.values.as_deref().unwrap_or(&[]).join("、")
                        ));
                    }
                }
            }
        }
    }
    // 试渲染：模板写错在这里暴露，不用等封存时报 G_RENDER_FAILED。
    let scan = crate::sample::scan(domain);
    let state = crate::sample::state();
    let input = crate::render::RenderInput { scan: &scan, state: &state, pack, policy_text: None };
    match crate::render::render_all(&set, &input) {
        Ok(out) if out.is_empty() => problems.push("没有任何输出".into()),
        Ok(_) => {}
        Err(e) => problems.push(format!("试渲染失败：{e}")),
    }
    problems
}

/// `packs check` 的对象：一个独立的域包目录。
pub struct Checked {
    pub domain: String,
    pub pack: Pack,
    pub problems: Vec<String>,
}

/// 验证一个域包目录（放进去之前查）。目录名就是域名。有问题不报错，列在 `problems` 里。
pub fn check_dir(dir: &Path) -> KResult<Checked> {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !valid_domain_name(&name) {
        return Err(KError::invalid_input(format!("目录名 {name:?} 不是合法的域名（小写字母、数字、连字符；以它作为域名）")));
    }
    let files = read_dir_files(dir);
    let pack = assemble(&files, &name)?;
    let problems = check(&pack, &files, &name);
    Ok(Checked { domain: name, pack, problems })
}

/// 验证一个目录作为某个域的项目级覆盖：按"只能收紧"的规则合并后再查。
pub fn check_overlay(home: &Home, root: &Path, base: &str, dir: &Path) -> KResult<Checked> {
    let b = resolve(home, root, base)?;
    let over = read_dir_files(dir);
    if over.contains_key("pack.json") {
        return Err(KError::new("E_PACK_INVALID", "覆盖里不能有 pack.json：项目级不能改规则，要改就新起一个域名"));
    }
    if over.is_empty() {
        return Err(KError::invalid_input(format!("{} 里没有可用的文件（只读 pack.json、templates/、method/）", dir.display())));
    }
    let mut files = b.files.clone();
    merge_restricted(&mut files, &over)?;
    let pack = assemble(&files, base)?;
    let problems = check(&pack, &files, base);
    Ok(Checked { domain: base.into(), pack, problems })
}

fn unknown_domain(home: &Home, root: &Path, domain: &str) -> KError {
    let list = describe(home, root);
    let text: Vec<String> = list
        .iter()
        .map(|d| {
            let desc = d["description"].as_str().unwrap_or("域包有问题，见 packs list");
            format!("{}（{desc}）", d["domain"].as_str().unwrap_or(""))
        })
        .collect();
    KError::new(
        "E_DOMAIN_UNKNOWN",
        format!("未知的域 {domain:?}。可用的域：{}。都没有合适的，可以自己做一个，见技能的 references/domain-pack.md", text.join("、")),
    )
    .with_data(json!({ "domains": list }))
}

// ─── 列出、查看、导出 ────────────────────────────────────────────────────────

/// 每个可用（或出问题）的域：名字、描述、来源、各层、叠加了什么。给 `packs list`、`info`、
/// 服务说明和未知域的报错用。
pub fn describe(home: &Home, root: &Path) -> Vec<Value> {
    domain_names(home, root)
        .into_iter()
        .map(|name| {
            let levels = levels_of(home, root, &name);
            match resolve(home, root, &name) {
                Ok(r) => json!({
                    "domain": name, "description": r.pack.description, "source": r.meta.source,
                    "digest": r.meta.digest, "levels": levels, "overlays": r.meta.overlays,
                }),
                Err(e) => json!({
                    "domain": name, "description": Value::Null, "levels": levels,
                    "error": { "code": e.code, "message": e.message },
                }),
            }
        })
        .collect()
}

/// 一个域的详情：合并后的规则、每个文件来自哪一层、方法文件。
pub fn show(home: &Home, root: &Path, domain: &str) -> KResult<Value> {
    let r = resolve(home, root, domain)?;
    let per_level: Vec<BTreeMap<String, String>> = Level::ALL.iter().map(|l| level_files(*l, home, root, domain)).collect();
    let files: Vec<Value> = r
        .files
        .keys()
        .map(|p| {
            let level = Level::ALL.iter().rev().find(|l| per_level[Level::ALL.iter().position(|x| x == *l).unwrap()].contains_key(p)).map(|l| l.name()).unwrap_or("");
            json!({ "path": p, "level": level })
        })
        .collect();
    let method: Vec<Value> = METHOD_STAGES.iter().filter_map(|s| r.pack.method.get(*s).map(|p| json!({ "stage": s, "path": p }))).collect();
    Ok(json!({
        "domain": domain, "description": r.pack.description, "source": r.meta.source,
        "digest": r.meta.digest, "overlays": r.meta.overlays,
        "rules": r.pack.to_value(), "method": method, "references": r.pack.references, "files": files,
    }))
}

/// 把合并后的域包写到一个空目录（或还不存在的目录），作为新域的起点。返回写出的文件。
pub fn export(dir: &Path, r: &Resolved) -> KResult<Vec<String>> {
    if dir.exists() && fs::read_dir(dir).map_err(crate::store::io_err)?.next().is_some() {
        return Err(KError::new("E_PACK_INVALID", format!("{} 不是空目录；导出会覆盖同名文件，换一个目录", dir.display())));
    }
    let mut written = Vec::new();
    for (p, c) in &r.files {
        let full = dir.join(p);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).map_err(crate::store::io_err)?;
        }
        fs::write(&full, c).map_err(crate::store::io_err)?;
        written.push(p.clone());
    }
    Ok(written)
}

// ─── 快照 ────────────────────────────────────────────────────────────────────

/// 扫描开始时写：`pack.json` 和方法文件（模板集另有自己的快照目录）。
pub fn write_snapshot(dir: &Path, r: &Resolved) -> KResult<()> {
    fs::create_dir_all(dir).map_err(crate::store::io_err)?;
    for (p, c) in &r.files {
        if p.starts_with("templates/") {
            continue;
        }
        let full = dir.join(p);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).map_err(crate::store::io_err)?;
        }
        fs::write(&full, c).map_err(crate::store::io_err)?;
    }
    Ok(())
}

/// 扫描过程中读快照。没有快照（这一版之前建的扫描）就按三层重新解析。
pub fn load_snapshot(dir: &Path, home: &Home, root: &Path, domain: &str) -> KResult<Pack> {
    if dir.join("pack.json").is_file() {
        let files = read_dir_files(dir);
        return assemble(&files, domain);
    }
    Ok(resolve(home, root, domain)?.pack)
}

// ─── 技能 ────────────────────────────────────────────────────────────────────

/// 通用技能：(相对路径, 内容)。只有一个，域方法不在技能里。
pub fn skill_files() -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk_embedded(&SKILL, "", &mut out);
    out
}

/// 技能名就是它的目录名。
pub const SKILL_NAME: &str = "project-scanner";

pub fn skill_description() -> String {
    SKILL.get_file("SKILL.md").and_then(|f| f.contents_utf8()).and_then(parse_front_description).unwrap_or_default()
}

/// 把技能写到目录（一个子目录）。返回技能名。
pub fn export_skills(dest: &Path) -> anyhow::Result<Vec<String>> {
    let name = SKILL_NAME.to_string();
    for (rel, content) in skill_files() {
        let p = dest.join(&name).join(&rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&p, content)?;
    }
    Ok(vec![name])
}

fn parse_front_description(md: &str) -> Option<String> {
    let rest = md.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    for line in rest[..end].lines() {
        if let Some(d) = line.trim().strip_prefix("description:") {
            return Some(d.trim().to_string());
        }
    }
    None
}
