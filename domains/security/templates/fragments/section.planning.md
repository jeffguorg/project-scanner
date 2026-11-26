---
type: fragment
select: { record: section, stage: planning }
fields:
  label: { type: text, required: true, max: 80 }
  kind: { type: enum, values: [attack-surface, baseline] }
  paths: { type: list, required: true, min: 1 }
  questions: { type: list, required: true, min: 1 }
---
板块 {{ section.id }} {{ section.label }}（{{ section.kind }}）
范围：{% for p in section.paths %}`{{ p }}`{% if not loop.last %}、{% endif %}{% endfor %}
问题：
{% for q in section.questions %}
- {{ q }}
{% endfor %}
