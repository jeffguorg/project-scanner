# project-scanner

项目扫描器：**agent 扫描，内核记录、过关卡、渲染报告。**

给 agent 四样东西——任务契约、方法论、状态与检查点、一份必须满足才能结束的放行条件。
安全扫描、项目 wiki、问题调研共用一个内核，内核之上是三个域包。

## 结构

```
crates/kernel    内核：存储、关卡、定位、身份、记录、渲染、进度（库）
crates/cli       可执行：project-scanner（命令行只读/管理）+ project-scanner mcp（给 agent 的服务）
domains/         域包：security / wiki / investigation（规则 + 模板 + 方法）
skills/          通用技能 project-scanner（SKILL.md + references）
```

## 构建与测试

```sh
cargo build --workspace
cargo test --workspace
```

## 用法

```sh
project-scanner info                       # 版本、数据目录、域包、技能、接入方式
project-scanner mcp                        # 启动 MCP stdio 服务（给宿主登记用）
project-scanner skills export <目录>        # 导出通用技能
project-scanner packs list|show|check|export # 域包
project-scanner scans list|show|log|export|render|delete
```

MCP 工具接口（agent 的唯一写入口）：`scan_start`、`scan_context`、`scan_templates`、
`scan_record`、`scan_progress`。



## License

Apache-2.0
