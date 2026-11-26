---
type: fragment
select: { record: section, stage: planning }
fields:
  label: { type: text, required: true, max: 80 }
  kind: { type: enum, values: [module, layer, entry, cross-cutting] }
  paths: { type: list, required: true, min: 1 }
  questions: { type: list, required: true, min: 1 }
---
# {{ section.label }}

范围：{% for p in section.paths %}`{{ p }}`{% if not loop.last %}、{% endif %}{% endfor %}

## 概述

- 待说明的问题：
{% for q in section.questions %}
- {{ q }}
{% endfor %}
- 断言：见各板块页
