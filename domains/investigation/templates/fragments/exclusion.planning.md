---
type: fragment
select: { record: exclusion, stage: planning }
fields:
  path: { type: text, required: true }
  reason: { type: text, required: true, max: 500 }
---
- `{{ path }}`：{{ reason }}
