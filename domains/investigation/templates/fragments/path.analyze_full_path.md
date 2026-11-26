---
type: fragment
select: { record: path, stage: analyze_full_path }
fields:
  # 步骤的结构（起点、结果、每步是定位或带理由的缺口）由内核校验，不在这里声明。
  prerequisites: { type: list, max: 20 }
---
（路径片段只加约束；渲染见 REPORT.md）
