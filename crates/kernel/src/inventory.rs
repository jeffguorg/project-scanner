//! preflight：定位目标、固定修订和内容摘要、清点范围、读策略。以及覆盖的计算。

use crate::error::{KError, KResult};
use crate::model::{Exclusion, Section};
use crate::pack::Pack;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvFile {
    pub path: String,
    pub size: u64,
}

/// preflight 的文件清单：范围内的文本文件，已去掉默认排除和策略排除。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub root: String,
    pub revision: String,
    pub dirty: bool,
    pub files: Vec<InvFile>,
    #[serde(default)]
    pub default_exclusions: Vec<String>,
    /// 依赖目录、构建产物等之外的排除：策略文件给的。
    #[serde(default)]
    pub policy_exclusions: Vec<String>,
    #[serde(default)]
    pub skipped_binary: u32,
}

impl Inventory {
    pub fn contains_path(&self, p: &str) -> bool {
        let p = p.trim_end_matches('/');
        self.files.iter().any(|f| f.path == p || f.path.starts_with(&format!("{p}/")))
    }
}

pub struct Preflight {
    pub revision: String,
    pub dirty: bool,
    pub remote: Option<String>,
    pub inventory: Inventory,
    pub snapshot_digest: Option<String>,
}

fn git(root: &Path, args: &[&str]) -> Result<String, KError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| KError::new("E_NOT_GIT", format!("运行 git 失败：{e}")))?;
    if !out.status.success() {
        return Err(KError::new(
            "E_NOT_GIT",
            format!("git {} 失败：{}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn is_binary(path: &Path) -> bool {
    match fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            let mut buf = [0u8; 8192];
            match f.read(&mut buf) {
                Ok(n) => buf[..n].contains(&0),
                Err(_) => true,
            }
        }
        Err(_) => true,
    }
}

fn norm(p: &str) -> String {
    let p = p.trim().trim_start_matches("./");
    p.trim_end_matches('/').to_string()
}

pub fn excluded_by(path: &str, prefixes: &[String]) -> bool {
    prefixes.iter().any(|p| {
        let p = norm(p);
        !p.is_empty() && (path == p || path.starts_with(&format!("{p}/")))
    })
}

/// 文件是否被某板块的范围覆盖：路径等于文件，或是它的目录前缀。
pub fn is_covered(path: &str, section_paths: &[String]) -> bool {
    section_paths.iter().any(|p| {
        let p = norm(p);
        !p.is_empty() && (path == p || path.starts_with(&format!("{p}/")))
    })
}

/// 范围内既没被活着的板块覆盖、也没被显式排除的文件。
pub fn uncovered<'a>(inv: &Inventory, sections: impl Iterator<Item = &'a Section>, exclusions: &[Exclusion]) -> Vec<String> {
    let mut cover: Vec<String> = Vec::new();
    for s in sections.filter(|s| s.is_live()) {
        cover.extend(s.paths.iter().cloned());
    }
    let ex: Vec<String> = exclusions.iter().map(|e| e.path.clone()).collect();
    inv.files
        .iter()
        .filter(|f| !is_covered(&f.path, &cover) && !excluded_by(&f.path, &ex))
        .map(|f| f.path.clone())
        .collect()
}

pub fn run(root: &Path, pack: &Pack, policy_exclusions: &[String]) -> KResult<Preflight> {
    git(root, &["rev-parse", "--is-inside-work-tree"]).map_err(|mut e| {
        if e.code == "E_NOT_GIT" {
            e.message = format!("{} 不是 git 仓库：{}", root.display(), e.message);
        }
        e
    })?;
    let revision = git(root, &["rev-parse", "HEAD"])?;
    let revision = revision.trim().to_string();
    if revision.is_empty() {
        return Err(KError::new("E_NOT_GIT", "仓库没有任何提交（HEAD 为空）"));
    }
    let status = git(root, &["status", "--porcelain"])?;
    let dirty = !status.trim().is_empty();
    let remote = git(root, &["remote", "get-url", "origin"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let listed = git(root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"])?;
    let mut prefixes: Vec<String> = pack.default_exclusions.clone();
    prefixes.extend(policy_exclusions.iter().cloned());
    let mut files: Vec<InvFile> = Vec::new();
    let mut skipped_binary = 0u32;
    for raw in listed.split('\0') {
        let raw = raw.trim();
        if raw.is_empty() || excluded_by(raw, &prefixes) || Some(raw) == pack.policy_file.as_deref() {
            continue;
        }
        let full = root.join(raw);
        let Ok(meta) = fs::symlink_metadata(&full) else {
            // 已删除但未提交的文件
            continue;
        };
        if !meta.file_type().is_file() {
            continue;
        }
        if is_binary(&full) {
            skipped_binary += 1;
            continue;
        }
        files.push(InvFile { path: raw.to_string(), size: meta.len() });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let snapshot_digest = if dirty { Some(snapshot_digest(root, &files)) } else { None };
    Ok(Preflight {
        revision,
        dirty,
        remote,
        inventory: Inventory {
            root: root.to_string_lossy().into_owned(),
            revision: String::new(),
            dirty,
            files,
            default_exclusions: pack.default_exclusions.clone(),
            policy_exclusions: policy_exclusions.to_vec(),
            skipped_binary,
        },
        snapshot_digest,
    })
}

fn snapshot_digest(root: &Path, files: &[InvFile]) -> String {
    let mut h = Sha256::new();
    for f in files {
        h.update(f.path.as_bytes());
        h.update(b"\0");
        match fs::read(root.join(&f.path)) {
            Ok(bytes) => {
                h.update(Sha256::digest(&bytes));
            }
            Err(_) => h.update(b"unreadable"),
        }
        h.update(b"\n");
    }
    hex::encode(h.finalize())
}

/// 策略文件：域包规定的文件名，放在根目录。扫描开始后冻结。
pub fn read_policy(root: &Path, pack: &Pack) -> Option<(String, String)> {
    let name = pack.policy_file.as_deref()?;
    let path: PathBuf = root.join(name);
    let text = fs::read_to_string(&path).ok()?;
    Some((name.to_string(), text))
}
