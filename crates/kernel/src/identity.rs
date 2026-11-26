//! 身份：判断两条断言是不是"同一条"的依据，跨修订、跨扫描稳定。
//!
//! `fingerprint = sha256(targetKey \n domain \n kind \n identity.anchor \n identity.instance ?? "")`

use crate::sha256_hex;

pub fn validate_anchor_slug(s: &str) -> Result<(), String> {
    if s.is_empty() || s.len() > 64 {
        return Err(format!("语义锚 {s:?} 的长度应在 1 到 64 之间"));
    }
    let ok = s.split('-').all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    if !ok {
        return Err(format!(
            "语义锚 {s:?} 只能是小写字母、数字，用连字符分段，例如 archive-extract-without-containment"
        ));
    }
    Ok(())
}

pub fn validate_instance(s: &str) -> Result<(), String> {
    if s.trim().is_empty() {
        return Err("instance 不能是空白".into());
    }
    if s.len() > 64 {
        return Err("instance 不能超过 64 个字符".into());
    }
    Ok(())
}

pub fn fingerprint(target_key: &str, domain: &str, kind: &str, anchor: &str, instance: Option<&str>) -> String {
    let mut data = String::new();
    data.push_str(target_key);
    data.push('\n');
    data.push_str(domain);
    data.push('\n');
    data.push_str(kind);
    data.push('\n');
    data.push_str(anchor);
    data.push('\n');
    data.push_str(instance.unwrap_or(""));
    sha256_hex(data.as_bytes())
}
