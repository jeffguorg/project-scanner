---
name: project-scanner
description: 对代码仓库做系统性的项目扫描，由 project-scanner 的工具记录、过关卡、封存成报告。用户要求安全审计、找漏洞、写项目文档或 wiki、调研某个问题的根因，或要一种新的扫描类型时使用。不要用于普通的代码阅读、单文件修改或 README 编辑。
---

# 项目扫描

扫描器不扫描，你才是扫描器。工具只做四件事：记检查点、算覆盖、过关卡、渲染报告。
按下面的顺序走：每一步的放行条件由内核检查，`scan_context` 返回的 `gaps` 和 `nextStep` 原样照做，当前阶段的做法在同一个返回的 `method` 里。

## 发起

1. 选域。内置三个域，按用户的要求挑最贴近的一个：
   - `security`：找可利用的安全问题。
   - `wiki`：生成项目文档，首页加每个板块一页。
   - `investigation`：调研一个问题的根因。

   每个域的一句话描述在服务接入时的说明和 `project-scanner info` 里；项目还可以带自己的域。都不贴合、用户又想要新的扫描类型时，读 `references/domain-pack.md` 做一个新域。
2. 把用户的要求**原话**作为 `prompt` 调 `scan_start`，`domain` 填选定的域，`root` 是目标目录（必须是 git 仓库）。范围和重点都在原话里，不要改写、不要省略。
3. 调 `scan_context`，进入 planning。

## 流程

进入 discovery、validation、analyze_full_path 由内核在放行条件满足时自动推进；finalized 要声明：

| 阶段 | 你做什么 | 过关的条件（内核算，缺的在 `gaps` 里） |
|---|---|---|
| planning | 读 `inventory`、`policy`、`pack`、`method`；把项目切成板块（`section.create`），每个板块有范围和待查问题；不查的目录用 `exclusion.add` 带理由排除 | 至少一个板块；每个文件属于某板块或被排除 |
| discovery | 逐板块调查，有一条记一条 `finding.create`：类别取自 `pack.findingKinds`，带语义锚和定位；没问题的板块 `disposition.set` | 每个板块有断言或处置；没有带警告的断言 |
| validation | 逐条 `verdict.attach`：对着源码证实或推翻，证实的写方法、证据、反证、前提、评级 | 每条断言有结论；证实的没有警告 |
| analyze_full_path | 给需要路径的证实断言 `path.attach`：从起点到结果，每步一个定位或写明缺口 | 路径完整 |
| finalized | `scan_progress { advance: "finalized" }` | 全部成立，渲染成功，封存 |

- 字段怎么填看 `scan_context` 的 `fragments`（字段表）；本阶段怎么做得好看 `method`（域包给的）；`references` 列出的细则用 `scan_context { view: "pack" }` 读。
- 三种后果：结构性错误（定位无效、身份冲突、阶段不允许）不写入，改了重提；缺字段是警告，记录已保存，进下一阶段前必须清；声明推进不过，阶段不变，按 `gaps` 补。
- 被压缩后先读 `scan_context` 的默认视图，再按需读记录。

## 硬规则

- 只读。扫描期间不改项目文件；修复是封存之后由人决定的事。
- 证据先于结论：先有定位，再有陈述。定位写成 `<修订>[+dirty]:<路径>:block_start(<行文本>)`，修订取 `scan_context` 里的 `scan.revision`；行文本选稳定的一行（函数定义、关键调用），不要选注释和空行。
- 先存再合并：有一条算一条，立刻提交，不要攒着。
- 复杂板块用全新上下文的子任务去审，子任务只把结果交回，不直接调工具。
- 发现新问题先 `scan_progress { reopen: [板块] }`，再在那个板块新建断言。
- 需要人决定的事：`scan_progress { waiting: { question } }`，停下来等回答；回答用 `appendRequest` 原话追加。
- 收到警告改了再提交。`W_ANCHOR_STALE` 表示代码变了，重新定位后再 `finding.update`。

## 收尾与恢复

- 封存后，人用 `project-scanner scans render <id> <目录>` 取报告；要改就从它起一次新扫描。
- 恢复未封存的扫描：`scan_start { resume }`，然后读 `scan_context` 的 `gaps`，从缺口处继续。
- 增量扫描（`incrementalFrom`）这一版还不支持，先全量。
