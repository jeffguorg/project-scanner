//! 存储：`PSCAN_HOME/targets/<targetKey>/scans/<scanId>/`，JSON 文件，flock，原子替换。

use crate::error::{KError, KResult};
use crate::inventory::Inventory;
use crate::model::{Progress, Scan, State};
use anyhow::{Context, Result};
use rustix::fs::{flock, FlockOperation};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

pub fn default_home() -> PathBuf {
    if let Ok(h) = std::env::var("PSCAN_HOME") {
        if !h.trim().is_empty() {
            return PathBuf::from(shellexpand_tilde(&h));
        }
    }
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| PathBuf::from(shellexpand_tilde(&s)))
        .unwrap_or_else(|| PathBuf::from(shellexpand_tilde("~/.local/share")));
    base.join("project-scanner")
}

fn shellexpand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest).to_string_lossy().into_owned();
        }
    }
    s.to_string()
}

#[derive(Debug, Clone)]
pub struct Home {
    pub root: PathBuf,
}

impl Home {
    pub fn detect() -> Home {
        Home { root: default_home() }
    }

    pub fn targets_dir(&self) -> PathBuf {
        self.root.join("targets")
    }

    pub fn target_dir(&self, key: &str) -> PathBuf {
        self.targets_dir().join(key)
    }

    pub fn packs_dir(&self) -> PathBuf {
        self.root.join("packs")
    }

    /// 所有扫描：(targetKey, scanId, 目录)，按 scanId 排序（scanId 时间有序）。
    pub fn all_scans(&self) -> Vec<(String, String, PathBuf)> {
        let mut out = Vec::new();
        let Ok(targets) = fs::read_dir(self.targets_dir()) else {
            return out;
        };
        for t in targets.flatten() {
            let key = t.file_name().to_string_lossy().into_owned();
            let Ok(scans) = fs::read_dir(t.path().join("scans")) else {
                continue;
            };
            for s in scans.flatten() {
                if s.path().join("scan.json").is_file() {
                    let id = s.file_name().to_string_lossy().into_owned();
                    out.push((key.clone(), id, s.path()));
                }
            }
        }
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }

    pub fn find_scan(&self, id: &str) -> Option<(String, PathBuf)> {
        if !valid_scan_id(id) {
            return None;
        }
        self.all_scans().into_iter().find(|(_, i, _)| i == id).map(|(k, _, p)| (k, p))
    }
}

pub fn valid_scan_id(id: &str) -> bool {
    id.len() == 12 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

#[derive(Debug, Clone)]
pub struct ScanStore {
    pub home: Home,
    pub target_key: String,
    pub scan_id: String,
    pub dir: PathBuf,
}

impl ScanStore {
    pub fn open(home: Home, id: &str) -> KResult<ScanStore> {
        let (key, dir) = home
            .find_scan(id)
            .ok_or_else(|| KError::new("E_SCAN_NOT_FOUND", format!("扫描 {id} 不存在")))?;
        Ok(ScanStore { home, target_key: key, scan_id: id.to_string(), dir })
    }

    pub fn create(home: Home, target_key: &str, id: &str) -> Result<ScanStore> {
        let dir = home.target_dir(target_key).join("scans").join(id);
        fs::create_dir_all(dir.join("checkpoints"))?;
        fs::create_dir_all(dir.join("templates"))?;
        let target_json = home.target_dir(target_key).join("target.json");
        if !target_json.exists() {
            fs::write(target_json, "{}\n")?;
        }
        Ok(ScanStore { home, target_key: target_key.to_string(), scan_id: id.to_string(), dir })
    }

    pub fn scan_json(&self) -> PathBuf {
        self.dir.join("scan.json")
    }
    pub fn state_json(&self) -> PathBuf {
        self.dir.join("state.json")
    }
    pub fn progress_json(&self) -> PathBuf {
        self.dir.join("progress.json")
    }
    pub fn inventory_json(&self) -> PathBuf {
        self.dir.join("inventory.json")
    }
    pub fn templates_dir(&self) -> PathBuf {
        self.dir.join("templates")
    }
    pub fn checkpoints_dir(&self) -> PathBuf {
        self.dir.join("checkpoints")
    }
    pub fn checkpoint_head(&self) -> PathBuf {
        self.dir.join("checkpoint-head.json")
    }
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("log.jsonl")
    }
    pub fn lock_path(&self) -> PathBuf {
        self.dir.join("lock")
    }
    pub fn sealed_dir(&self) -> PathBuf {
        self.dir.join("sealed")
    }
    pub fn policy_path(&self) -> PathBuf {
        self.dir.join("policy.md")
    }
    /// 开始时生效的域包快照（`pack.json` + `method/**`）。
    pub fn pack_dir(&self) -> PathBuf {
        self.dir.join("pack")
    }

    pub fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
        let bytes = fs::read(path).with_context(|| format!("读 {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("解析 {}", path.display()))
    }

    /// 原子写：先写同目录临时文件，再 rename。
    pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
        let data = serde_json::to_vec_pretty(value)?;
        fs::write(&tmp, &data).with_context(|| format!("写 {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("替换 {}", path.display()))?;
        Ok(())
    }

    pub fn read_scan(&self) -> KResult<Scan> {
        Self::read(&self.scan_json()).map_err(io_err)
    }
    pub fn write_scan(&self, scan: &Scan) -> KResult<()> {
        Self::write_json(&self.scan_json(), scan).map_err(io_err)
    }
    pub fn read_state(&self) -> KResult<State> {
        Self::read(&self.state_json()).map_err(io_err)
    }
    pub fn write_state(&self, state: &State) -> KResult<()> {
        Self::write_json(&self.state_json(), state).map_err(io_err)
    }
    pub fn read_progress(&self) -> KResult<Progress> {
        if self.progress_json().exists() {
            Self::read(&self.progress_json()).map_err(io_err)
        } else {
            Ok(Progress::default())
        }
    }
    pub fn write_progress(&self, progress: &Progress) -> KResult<()> {
        Self::write_json(&self.progress_json(), progress).map_err(io_err)
    }
    pub fn read_inventory(&self) -> KResult<Inventory> {
        Self::read(&self.inventory_json()).map_err(io_err)
    }

    /// flock：写操作排他，读操作共享，操作结束就释放。进程退出锁自动释放。
    pub fn locked<T>(&self, exclusive: bool, f: impl FnOnce() -> T) -> Result<T> {
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(self.lock_path())?;
        flock(&file, if exclusive { FlockOperation::LockExclusive } else { FlockOperation::LockShared })?;
        let out = f();
        let _ = flock(&file, FlockOperation::Unlock);
        Ok(out)
    }

    pub fn is_sealed(&self) -> bool {
        self.sealed_dir().join("manifest.json").is_file()
    }
}

pub fn io_err(e: impl Into<anyhow::Error>) -> KError {
    let e = e.into();
    KError::new("E_IO", format!("{e:#}"))
}

/// scanId：时间有序的 12 位 base32 小写（48 位毫秒时间戳 + 12 位随机）。
pub fn new_scan_id() -> String {
    const ALPHA: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    let ms = chrono::Utc::now().timestamp_millis() as u64 & 0xFFFF_FFFF_FFFF;
    let u = uuid::Uuid::new_v4();
    let rnd = (u.as_bytes()[0] as u64 & 0x0F) << 8 | u.as_bytes()[1] as u64;
    let mut n = (ms << 12) | rnd;
    let mut out = String::with_capacity(12);
    for _ in 0..12 {
        out.push(ALPHA[(n & 31) as usize] as char);
        n >>= 5;
    }
    out.chars().rev().collect()
}

/// targetKey：规范化的远端地址或绝对路径的 sha256 前 16 位。
pub fn target_key(remote: Option<&str>, root: &Path) -> String {
    let basis = remote.map(|r| r.to_string()).unwrap_or_else(|| root.to_string_lossy().into_owned());
    crate::sha256_hex(basis.as_bytes())[..16].to_string()
}

pub fn writer_check(scan: &Scan, session_id: &str) -> KResult<()> {
    if scan.writer.session_id != session_id {
        return Err(KError::new("E_NOT_WRITER", crate::text::e_not_writer(&scan.writer.since))
            .with_data(json!({ "writer": scan.writer })));
    }
    Ok(())
}
