//! 定位：`<commit>[+dirty]:<relpath>:block_start(<line_text>)[:block_end(<line_text>)][:+<offset>]`
//!
//! 解析时记录引用了哪些行、这些行的内容摘要。重新解析时摘要不一致或解析不了就是过期。

use crate::model::AnchorRef;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub commit: String,
    pub dirty: bool,
    pub relpath: String,
    pub block_start: String,
    pub block_end: Option<String>,
    pub offset: Option<i32>,
}

/// 找到闭合 block_start 的那个右括号：它后面的内容只能是空、`:+偏移` 或 `:block_end(`。
fn split_close(inner: &str) -> Result<(String, String), String> {
    for (i, ch) in inner.char_indices() {
        if ch == ')' {
            let tail = &inner[i + ch.len_utf8()..];
            if tail.is_empty() || tail.starts_with(":+") || tail.starts_with(":block_end(") {
                return Ok((inner[..i].to_string(), tail.to_string()));
            }
        }
    }
    Err("括号没有闭合".into())
}

pub fn parse(s: &str) -> Result<Anchor, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("定位为空".into());
    }
    let (head, rest) = s.split_once(':').ok_or_else(|| "修订后缺少 ':'".to_string())?;
    let (commit, dirty) = match head.strip_suffix("+dirty") {
        Some(c) => (c, true),
        None => (head, false),
    };
    if commit.is_empty() {
        return Err("修订为空".into());
    }
    let (relpath, inner) = rest.split_once(":block_start(").ok_or_else(|| "缺少 ':block_start('".to_string())?;
    if relpath.is_empty()
        || relpath.starts_with('/')
        || relpath.starts_with("./")
        || relpath.contains('\\')
        || relpath.contains("//")
        || relpath.ends_with('/')
        || relpath.split('/').any(|seg| seg == ".." || seg == ".")
    {
        return Err(format!("相对路径不合法：{relpath:?}"));
    }
    let (start, mut tail) = split_close(inner)?;
    let start = start.trim().to_string();
    if start.is_empty() {
        return Err("block_start 的文本为空".into());
    }
    if start.contains('\n') {
        return Err("文本不能含换行".into());
    }
    let mut block_end = None;
    let mut offset = None;
    if let Some(t) = tail.strip_prefix(":block_end(") {
        let (e, t2) = split_close(t)?;
        let e = e.trim().to_string();
        if e.is_empty() {
            return Err("block_end 的文本为空".into());
        }
        block_end = Some(e);
        tail = t2;
    }
    if let Some(o) = tail.strip_prefix(":+") {
        offset = Some(o.parse::<i32>().map_err(|_| format!("偏移不是整数：{o:?}"))?);
        tail = "".to_string();
    }
    if !tail.is_empty() {
        return Err(format!("多余的尾部内容：{tail:?}"));
    }
    Ok(Anchor {
        commit: commit.to_string(),
        dirty,
        relpath: relpath.to_string(),
        block_start: start,
        block_end,
        offset,
    })
}

pub fn digest_of(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub refs: Vec<AnchorRef>,
}

fn aref(original: &str, role: &str, idx: usize, line: &str) -> AnchorRef {
    AnchorRef {
        anchor: original.to_string(),
        role: role.to_string(),
        line: idx as u32 + 1,
        digest: digest_of(line.trim()),
    }
}

/// 对着工作区解析。找不到唯一行、文件读不到、不是文本，都是错误。
pub fn resolve(root: &Path, original: &str, a: &Anchor) -> Result<Resolved, String> {
    let path = root.join(&a.relpath);
    let bytes = fs::read(&path).map_err(|e| format!("读不到 {}：{e}", a.relpath))?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err(format!("{} 不是文本文件", a.relpath));
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Err(format!("{} 是空文件", a.relpath));
    }
    let find = |needle: &str, range: std::ops::Range<usize>| -> Result<usize, String> {
        let hits: Vec<usize> = range.filter(|&i| lines[i].trim().contains(needle)).collect();
        match hits.len() {
            1 => Ok(hits[0]),
            0 => Err(format!("{} 里没有一行包含 {:?}", a.relpath, needle)),
            n => Err(format!("{} 里有 {n} 行包含 {:?}，不唯一", a.relpath, needle)),
        }
    };
    let start = find(&a.block_start, 0..lines.len())?;
    let (end, ref_line) = match &a.block_end {
        Some(be) => {
            let end = find(be, start + 1..lines.len())?;
            let r = match a.offset {
                Some(o) => {
                    let r = start as i64 + o as i64;
                    if r < start as i64 || r > end as i64 {
                        return Err(format!("偏移 {o} 落在块外（第 {} 到 {} 行）", start + 1, end + 1));
                    }
                    r as usize
                }
                None => start,
            };
            (end, r)
        }
        None => {
            let r = match a.offset {
                Some(o) => {
                    let r = start as i64 + o as i64;
                    if r < 0 || r >= lines.len() as i64 {
                        return Err(format!("偏移 {o} 越出文件（共 {} 行）", lines.len()));
                    }
                    r as usize
                }
                None => start,
            };
            (start, r)
        }
    };
    let mut refs = vec![aref(original, "start", start, lines[start])];
    if end != start {
        refs.push(aref(original, "end", end, lines[end]));
    }
    if ref_line != start && ref_line != end {
        refs.push(aref(original, "ref", ref_line, lines[ref_line]));
    }
    Ok(Resolved { refs })
}

/// 两次解析的结果是否一致：只比角色和内容摘要，不比行号——增删行、无关改动之后仍然有效。
pub fn same_refs(a: &[AnchorRef], b: &[AnchorRef]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(x, y)| x.role == y.role && x.digest == y.digest)
}
