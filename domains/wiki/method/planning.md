# planning：画项目地图

- 读上下文里的 `inventory` 和用户要求。按模块、分层、入口切板块（`kind: module | layer | entry | cross-cutting`），以代码的实际边界为准（目录、构建单元、进程），不要按文档的章节切。
- 每个板块的 `questions` 是"这一页要回答什么"：它做什么、怎么被调用、依赖谁、有哪些不变量。
- 生成的代码、第三方、和文档无关的目录用 `exclusion.add` 带理由排除。
