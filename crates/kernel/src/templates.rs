//! 模板集：片段（声明字段约束 + Jinja 正文）和输出（key + 选择器）。
//! 校验和渲染同源：校验读前置元数据，渲染读正文。

use crate::error::{KError, KResult};
use crate::model::{Phase, Warning};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub const FIELD_TYPES: [&str; 6] = ["text", "enum", "anchors", "refs", "list", "count"];
pub const RECORD_TYPES: [&str; 4] = ["section", "finding", "path", "exclusion"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldDecl {
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// 字段声明表：保持声明顺序，序列化成映射。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FieldDecls(pub Vec<(String, FieldDecl)>);

impl FieldDecls {
    pub fn get(&self, k: &str) -> Option<&FieldDecl> {
        self.0.iter().find(|(n, _)| n == k).map(|(_, d)| d)
    }
    pub fn iter(&self) -> impl Iterator<Item = &(String, FieldDecl)> {
        self.0.iter()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for FieldDecls {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = FieldDecls;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("字段声明表")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<FieldDecls, A::Error> {
                let mut out = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, FieldDecl>()? {
                    out.push((k, v));
                }
                Ok(FieldDecls(out))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<FieldDecls, A::Error> {
                let mut out = Vec::new();
                while let Some(pair) = seq.next_element::<(String, FieldDecl)>()? {
                    out.push(pair);
                }
                Ok(FieldDecls(out))
            }
        }
        deserializer.deserialize_any(V)
    }
}

impl Serialize for FieldDecls {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FragmentSelect {
    pub record: String,
    pub stage: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Meta {
    Fragment {
        select: FragmentSelect,
        #[serde(default)]
        fields: FieldDecls,
    },
    Output {
        key: String,
        #[serde(default = "default_select")]
        select: String,
    },
}

fn default_select() -> String {
    "all".to_string()
}

impl Meta {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[derive(Debug, Clone)]
pub struct TemplateFile {
    pub path: String,
    pub meta: Meta,
    pub body: String,
    /// 原文（含 front matter）。写盘和算摘要都用它，读回来摘要不变。
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct TemplateSet {
    /// fragments/ 和 outputs/ 下的文件，按路径。
    pub files: BTreeMap<String, TemplateFile>,
    /// partials/ 下的文件：只能被 include。
    pub partials: BTreeMap<String, String>,
    pub digest: String,
}

fn norm_path(p: &str) -> String {
    p.trim().trim_start_matches("./").trim_start_matches('/').to_string()
}

pub(crate) fn split_front(content: &str) -> KResult<(&str, &str)> {
    let rest = content.strip_prefix("---\n").ok_or_else(|| KError::new("E_TEMPLATE_INVALID", "文件要以 '---' 开头的前置元数据开始"))?;
    let end = rest.find("\n---").ok_or_else(|| KError::new("E_TEMPLATE_INVALID", "前置元数据没有以一行 '---' 结束"))?;
    let front = &rest[..end];
    let body = rest[end + 4..].strip_prefix('\n').unwrap_or("");
    Ok((front, body))
}

pub fn parse_file(path: &str, content: &str) -> KResult<TemplateFile> {
    let path = norm_path(path);
    let (front, body) = split_front(content)?;
    let meta: Meta = serde_yaml_ng::from_str(front)
        .map_err(|e| KError::new("E_TEMPLATE_INVALID", format!("{path} 的前置元数据解析失败：{e}")))?;
    match &meta {
        Meta::Fragment { select, fields } => {
            if !path.starts_with("fragments/") {
                return Err(KError::new("E_TEMPLATE_INVALID", format!("{path}：片段要放在 fragments/ 下")));
            }
            if !RECORD_TYPES.contains(&select.record.as_str()) {
                return Err(KError::new("E_TEMPLATE_INVALID", format!("{path}：select.record 只能是 {}", RECORD_TYPES.join("、"))));
            }
            if fields.is_empty() {
                return Err(KError::new("E_TEMPLATE_INVALID", format!("{path}：片段至少声明一个字段")));
            }
            for (name, decl) in fields.iter() {
                if !FIELD_TYPES.contains(&decl.ty.as_str()) {
                    return Err(KError::new(
                        "E_TEMPLATE_INVALID",
                        format!("{path}：字段 {name} 的类型 {:?} 不认识，只能是 {}", decl.ty, FIELD_TYPES.join("、")),
                    ));
                }
                if decl.ty == "enum" && decl.values.as_ref().map_or(true, |v| v.is_empty()) {
                    return Err(KError::new("E_TEMPLATE_INVALID", format!("{path}：枚举字段 {name} 要给 values")));
                }
                if decl.when.as_ref().map_or(false, |w| w.is_empty()) {
                    return Err(KError::new("E_TEMPLATE_INVALID", format!("{path}：字段 {name} 的 when 要写 {{字段: 取值}}")));
                }
            }
        }
        Meta::Output { key, select } => {
            if !path.starts_with("outputs/") {
                return Err(KError::new("E_TEMPLATE_INVALID", format!("{path}：输出要放在 outputs/ 下")));
            }
            if key.trim().is_empty() || key.starts_with('/') || key.split('/').any(|s| s == "..") {
                return Err(KError::new("E_TEMPLATE_INVALID", format!("{path}：输出的 key 不合法：{key:?}")));
            }
            if !["all", "section", "sections"].contains(&select.as_str()) {
                return Err(KError::new("E_TEMPLATE_INVALID", format!("{path}：输出的 select 只能是 all、section、sections")));
            }
        }
    }
    Ok(TemplateFile { path, meta, body: body.to_string(), text: content.to_string() })
}

impl TemplateSet {
    pub fn build(files: Vec<(String, String)>) -> KResult<TemplateSet> {
        let mut set = TemplateSet::default();
        let mut sorted = files;
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let mut hash_input = String::new();
        for (path, content) in sorted {
            let path = norm_path(&path);
            hash_input.push_str(&path);
            hash_input.push('\0');
            hash_input.push_str(&content);
            hash_input.push('\n');
            if path.starts_with("partials/") {
                set.partials.insert(path, content);
            } else {
                let tf = parse_file(&path, &content)?;
                set.files.insert(path, tf);
            }
        }
        set.digest = crate::sha256_hex(hash_input.as_bytes());
        Ok(set)
    }

    /// 从磁盘目录读（扫描的 templates/ 快照）。
    pub fn read_dir(dir: &Path) -> KResult<TemplateSet> {
        let mut files = Vec::new();
        collect(dir, "", &mut files);
        Self::build(files).map_err(|e| KError::new(&e.code, format!("{}：{}", dir.display(), e.message)))
    }

    /// 写到磁盘目录（扫描创建时复制默认集）。
    pub fn write_dir(&self, dir: &Path) -> KResult<()> {
        for (path, tf) in &self.files {
            let full = dir.join(path);
            if let Some(p) = full.parent() {
                fs::create_dir_all(p).map_err(crate::store::io_err)?;
            }
            let text = tf.text.clone();
            fs::write(&full, text).map_err(crate::store::io_err)?;
        }
        for (path, content) in &self.partials {
            let full = dir.join(path);
            if let Some(p) = full.parent() {
                fs::create_dir_all(p).map_err(crate::store::io_err)?;
            }
            fs::write(&full, content).map_err(crate::store::io_err)?;
        }
        Ok(())
    }

    /// 某个文件的全文（前置元数据 + 正文）。
    pub fn file_text(&self, path: &str) -> Option<String> {
        if let Some(tf) = self.files.get(path) {
            return Some(tf.text.clone());
        }
        self.partials.get(path).cloned()
    }

    pub fn fragments(&self) -> Vec<&TemplateFile> {
        self.files.values().filter(|f| matches!(f.meta, Meta::Fragment { .. })).collect()
    }

    pub fn outputs(&self) -> Vec<&TemplateFile> {
        self.files.values().filter(|f| matches!(f.meta, Meta::Output { .. })).collect()
    }

    /// 每类记录在哪些阶段有片段（后一阶段包含前一阶段的要求）。
    pub fn stages_for(record: &str) -> &'static [Phase] {
        match record {
            "section" | "exclusion" => &[Phase::Planning],
            "path" => &[Phase::AnalyzeFullPath],
            _ => &[Phase::Discovery, Phase::Validation, Phase::AnalyzeFullPath],
        }
    }

    /// 最具体的片段：同阶段里 kind 匹配的优先。
    pub fn fragment_for(&self, record: &str, stage: Phase, kind: Option<&str>) -> Option<&TemplateFile> {
        let mut best: Option<&TemplateFile> = None;
        for f in self.fragments() {
            if let Meta::Fragment { select, .. } = &f.meta {
                if select.record == record && select.stage == stage {
                    match (select.kind.as_deref(), kind) {
                        (Some(k), Some(k2)) if k == k2 => return Some(f),
                        (None, _) if best.is_none() => best = Some(f),
                        _ => {}
                    }
                }
            }
        }
        best
    }

    /// 把一条记录绑到片段上：各阶段（≤ up_to）的片段全部检查，警告合并。
    pub fn bind(&self, record: &str, kind: Option<&str>, obj: &Map<String, Value>, up_to: Phase) -> Vec<Warning> {
        let mut out = Vec::new();
        let mut bound_any = false;
        let mut relevant_any = false;
        for stage in Self::stages_for(record) {
            if *stage > up_to {
                continue;
            }
            relevant_any = true;
            if let Some(f) = self.fragment_for(record, *stage, kind) {
                out.extend(bind_fields(f, obj, *stage));
                bound_any = true;
            }
        }
        if relevant_any && !bound_any {
            out.push(Warning {
                code: "W_NO_FRAGMENT".into(),
                field: String::new(),
                message: format!("{record} 类记录在 {} 之前没有可绑定的片段，无法校验", up_to),
                stage: None,
            });
        }
        out
    }

    /// 片段字段表：给 scan_context 和 scans show。
    pub fn field_table(&self, record: &str, stage: Phase) -> Option<Map<String, Value>> {
        let f = self.fragment_for(record, stage, None)?;
        let Meta::Fragment { fields, .. } = &f.meta else {
            return None;
        };
        let mut m = Map::new();
        for (name, decl) in fields.iter() {
            let mut d = json!({ "type": decl.ty });
            if let Some(v) = &decl.values {
                d["values"] = json!(v);
            }
            if let Some(r) = decl.required {
                d["required"] = json!(r);
            }
            if let Some(w) = &decl.when {
                d["when"] = json!(w);
            }
            if let Some(x) = decl.min {
                d["min"] = json!(x);
            }
            if let Some(x) = decl.max {
                d["max"] = json!(x);
            }
            m.insert(name.clone(), d);
        }
        Some(m)
    }

    /// 当前阶段各记录类型的字段表。
    pub fn field_tables(&self, stage: Phase) -> Map<String, Value> {
        let mut out = Map::new();
        for record in RECORD_TYPES {
            let relevant: Vec<Phase> = Self::stages_for(record).iter().copied().filter(|s| *s <= stage).collect();
            if let Some(last) = relevant.last() {
                if let Some(table) = self.field_table(record, *last) {
                    out.insert(record.to_string(), json!({ "stage": last, "fields": table }));
                }
            }
        }
        out
    }
}

fn collect(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = format!("{prefix}{name}");
        let p = e.path();
        if p.is_dir() {
            collect(&p, &format!("{rel}/"), out);
        } else if let Ok(s) = fs::read_to_string(&p) {
            out.push((rel, s));
        }
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

fn when_applies(decl: &FieldDecl, obj: &Map<String, Value>) -> bool {
    match &decl.when {
        Some(w) => w.iter().next().map(|(k, v)| obj.get(k) == Some(v)).unwrap_or(true),
        None => true,
    }
}

fn warn(code: &str, field: &str, message: String, stage: Phase) -> Warning {
    Warning { code: code.to_string(), field: field.to_string(), message, stage: Some(stage) }
}

pub fn bind_fields(f: &TemplateFile, obj: &Map<String, Value>, stage: Phase) -> Vec<Warning> {
    let Meta::Fragment { fields, .. } = &f.meta else {
        return vec![];
    };
    let mut out = Vec::new();
    for (name, decl) in fields.iter() {
        let value = obj.get(name).filter(|v| !is_empty(v));
        let applies = when_applies(decl, obj);
        let Some(v) = value else {
            if decl.required == Some(true) && applies {
                out.push(warn("W_REQUIRED_MISSING", name, format!("字段 {name} 必填"), stage));
            }
            continue;
        };
        if !applies {
            continue;
        }
        let len = |v: &Value| -> Option<usize> {
            match v {
                Value::String(s) => Some(s.chars().count()),
                Value::Array(a) => Some(a.len()),
                _ => None,
            }
        };
        match decl.ty.as_str() {
            "text" => match v {
                Value::String(_) => {}
                _ => out.push(warn("W_TYPE", name, format!("字段 {name} 应为文本"), stage)),
            },
            "enum" => match v {
                Value::String(s) => {
                    let values = decl.values.as_deref().unwrap_or(&[]);
                    if !values.contains(s) {
                        out.push(warn("W_ENUM", name, format!("字段 {name} 的取值 {s:?} 不在 {} 里", values.join("、")), stage));
                    }
                }
                _ => out.push(warn("W_TYPE", name, format!("字段 {name} 应为枚举值"), stage)),
            },
            "anchors" | "refs" | "list" => match v {
                Value::Array(a) => {
                    if a.iter().any(|x| !x.is_string()) {
                        out.push(warn("W_TYPE", name, format!("字段 {name} 的每一项应为文本"), stage));
                    }
                }
                _ => out.push(warn("W_TYPE", name, format!("字段 {name} 应为列表"), stage)),
            },
            "count" => match v {
                Value::Number(_) => {}
                _ => out.push(warn("W_TYPE", name, format!("字段 {name} 应为数字"), stage)),
            },
            _ => {}
        }
        if let Some(n) = len(v) {
            if let Some(max) = decl.max {
                if n as f64 > max {
                    out.push(warn("W_TOO_LONG", name, format!("字段 {name} 超过上限 {max}，现在 {n}"), stage));
                }
            }
            if let Some(min) = decl.min {
                if (n as f64) < min {
                    out.push(warn("W_TOO_FEW", name, format!("字段 {name} 至少 {min}，现在 {n}"), stage));
                }
            }
        }
    }
    out
}

/// 新片段比旧的松了就返回原因。
pub fn loosen(old: &TemplateFile, new: &TemplateFile) -> Option<String> {
    match (&old.meta, &new.meta) {
        (Meta::Fragment { select: s1, fields: f1 }, Meta::Fragment { select: s2, fields: f2 }) => {
            if s1 != s2 {
                return Some("select 变了：只能收紧字段，不能换记录类型、阶段或类别".into());
            }
            for (name, d1) in f1.iter() {
                let Some(d2) = f2.get(name) else {
                    return Some(format!("删掉了字段 {name}"));
                };
                if d1.ty != d2.ty {
                    return Some(format!("字段 {name} 的类型变了"));
                }
                if d1.required == Some(true) && d2.required != Some(true) {
                    return Some(format!("字段 {name} 的必填被放宽"));
                }
                match (&d1.values, &d2.values) {
                    (Some(_), None) => return Some(format!("字段 {name} 的枚举被放宽")),
                    (Some(v1), Some(v2)) => {
                        if !v2.iter().all(|v| v1.contains(v)) {
                            return Some(format!("字段 {name} 的枚举取值变多了"));
                        }
                    }
                    _ => {}
                }
                if let Some(m1) = d1.min {
                    if d2.min.map_or(true, |m2| m2 < m1) {
                        return Some(format!("字段 {name} 的下限被放宽"));
                    }
                }
                if let Some(m1) = d1.max {
                    if d2.max.map_or(true, |m2| m2 > m1) {
                        return Some(format!("字段 {name} 的上限被放宽"));
                    }
                }
                if d1.when.is_some() && d1.when != d2.when {
                    return Some(format!("字段 {name} 的 when 变了"));
                }
                if d1.when.is_none() && d2.when.is_some() {
                    return Some(format!("字段 {name} 加了 when，等于放宽"));
                }
            }
            None
        }
        _ => None,
    }
}
