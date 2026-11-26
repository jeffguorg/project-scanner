---
type: fragment
select: { record: section, stage: planning }
fields:
  label: { type: text, required: true, max: 80 }
  kind: { type: enum, values: [component, hypothesis-group, context] }
  paths: { type: list, required: true, min: 1 }
  questions: { type: list, required: true, min: 1 }
---
板块 {{ section.id }} {{ section.label }}（{{ section.kind }}）
