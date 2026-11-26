//! 渲染集成测试：三个域包各用内核的样例状态（crates/kernel/src/sample.rs）渲染全部输出，与
//! tests/snapshots 里的基准文件逐字比较。改了模板或渲染上下文、确认输出无误后，用
//! `PSCAN_UPDATE_SNAPSHOTS=1 cargo test -p project-scanner-kernel` 更新基准。
//!
//! 样例不经过内核流程，直接构造状态，所以能包含封存前不会同时出现的情况（未结论、搁置、重开）——
//! 渲染对任何状态都不能失败，否则 `G_RENDER_FAILED` 会盖住别的缺口。同一份样例也是 `packs check`
//! 试渲染用的那份。

use project_scanner_kernel::render::{render_all, RenderInput};
use project_scanner_kernel::store::Home;
use project_scanner_kernel::templates::TemplateSet;
use project_scanner_kernel::model::{Overlay, PackMeta};
use project_scanner_kernel::{pack, sample};

#[test]
fn every_pack_renders_the_same_state() {
    let tmp = std::env::temp_dir().join(format!("pscan-render-{}", std::process::id()));
    let home = Home { root: tmp.join("home") };
    // 没有项目级域包：根是一个空目录。
    let root = tmp.join("root");
    std::fs::create_dir_all(&root).unwrap();
    let snapshots = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let update = std::env::var("PSCAN_UPDATE_SNAPSHOTS").is_ok();
    std::fs::create_dir_all(&snapshots).unwrap();
    let state = sample::state();
    let policy = "# 策略\n\n- exclude: examples/\n\n什么重要：会话与租户隔离\n";

    let mut total = 0;
    for domain in pack::domain_names(&home, &root) {
        // 解析包含整套检查（规则、片段、试渲染）；这里只取合并后的模板集。
        let resolved = pack::resolve(&home, &root, &domain).unwrap_or_else(|e| panic!("{domain} 的域包不可用：{e}"));
        let set = TemplateSet::build(resolved.template_files()).unwrap();
        let dir = tmp.join(&domain);
        set.write_dir(&dir).unwrap();
        // 写盘再读回，摘要不变；扫描目录里的模板集就是这样的。
        assert_eq!(TemplateSet::read_dir(&dir).unwrap().digest, set.digest, "{domain} 的模板集写盘后摘要变了");

        let scan = sample::scan(&domain);
        let out = render_all(&set, &RenderInput { scan: &scan, state: &state, pack: &resolved.pack, policy_text: Some(policy) })
            .unwrap_or_else(|e| panic!("{domain} 渲染失败：{e}"));
        assert!(!out.is_empty(), "{domain} 没有输出");

        let mut keys: Vec<&str> = out.iter().map(|r| r.key.as_str()).collect();
        keys.sort();
        let expected: Vec<&str> = match domain.as_str() {
            "wiki" => vec!["INDEX.md", "上传.md", "依赖与配置.md", "结账.md", "认证.md", "请求处理流程.md", "配置.md"],
            _ => vec!["REPORT.md"],
        };
        assert_eq!(keys, expected, "{domain} 的输出 key 不对");

        for r in &out {
            let path = snapshots.join(format!("{domain}--{}", r.key));
            if update {
                std::fs::write(&path, &r.content).unwrap();
            }
            for bad in ["\n---\ntype:", "生成于 None", "\n\n\n"] {
                if r.content.contains(bad) {
                    // 更新模式下只报告，文件照样落盘，方便看是哪里出的问题。
                    assert!(update, "{domain}/{} 里出现了 {:?}（{}）", r.key, bad, path.display());
                    eprintln!("注意：{domain}/{} 里出现了 {:?}", r.key, bad);
                }
            }
            if !update {
                let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("缺基准文件 {}", path.display()));
                assert!(r.content == want, "{domain}/{} 的渲染结果变了（{}）；确认无误后用 PSCAN_UPDATE_SNAPSHOTS=1 更新基准", r.key, path.display());
            }
            total += 1;
        }
    }
    assert_eq!(total, 9, "security、investigation 各一份报告，wiki 一个导航页加六个板块页");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// 叠加了项目级的扫描，报告开头写明来源和叠了什么；没叠加就不写这一行（上面的基准就是）。
#[test]
fn report_header_names_overlaid_layers() {
    let tmp = std::env::temp_dir().join(format!("pscan-render-overlay-{}", std::process::id()));
    let home = Home { root: tmp.join("home") };
    let root = tmp.join("root");
    std::fs::create_dir_all(&root).unwrap();
    let resolved = pack::resolve(&home, &root, "security").unwrap();
    let set = TemplateSet::build(resolved.template_files()).unwrap();
    let mut scan = sample::scan("security");
    scan.pack = PackMeta {
        domain: "security".into(),
        source: "embedded".into(),
        overlays: vec![
            Overlay { level: "user".into(), files: vec!["method/planning.md".into(), "pack.json".into()] },
            Overlay { level: "project".into(), files: vec!["method/discovery.md".into()] },
        ],
        ..Default::default()
    };
    let state = sample::state();
    let out = render_all(&set, &RenderInput { scan: &scan, state: &state, pack: &resolved.pack, policy_text: None }).unwrap();
    let report = &out.iter().find(|r| r.key == "REPORT.md").unwrap().content;
    assert!(report.contains("- 域包：规则来自 embedded；叠加了 user（method/planning.md、pack.json），project（method/discovery.md）\n"), "{report}");
    let _ = std::fs::remove_dir_all(&tmp);
}