{#- 已证实的流程类断言放在“端到端流程”里，其余的都算“描述”。 -#}
{% set ns = namespace(shown = 0) %}
{% for f in s.findings if (f.kind != "flow" or f.status != "validated") and f.status != "retracted" %}
{% set ns.shown = ns.shown + 1 %}
{% endfor %}
{% if ns.shown > 0 %}
## 描述
{% for f in s.findings if (f.kind != "flow" or f.status != "validated") and f.status != "retracted" %}
{% if not loop.first %}

{% endif %}
{% if f.status == "validated" %}
- **{{ f.fields.title }}**（{{ f.id }}，{{ f.kind }}）：{{ f.fields.claim }}
  - 依据：`{{ (f.fields.evidence or []) | join("`、`") }}`
  - 置信度：{{ f.fields.confidence }}
{% elif f.status == "refuted" %}
- ~~{{ f.fields.title }}~~（{{ f.id }}）：已推翻——{{ f.fields.rationale }}
{% elif f.status == "deferred" %}
- {{ f.fields.title }}（{{ f.id }}）：搁置——{{ f.fields.rationale }}
{% elif f.status == "candidate" %}
- {{ f.fields.title }}（{{ f.id }}）：未结论——{{ f.fields.claim }}
{% endif %}
{% endfor %}
{% endif %}
{% set flows = s.findings | selectattr("status", "equalto", "validated") | selectattr("kind", "equalto", "flow") %}
{% if flows %}
## 端到端流程
{% for f in flows %}

### {{ f.fields.title }}（{{ f.id }}）

{{ f.fields.claim }}

{% for p in f.paths %}
{% for st in p.steps %}
- {{ st.role }}：{% if st.anchor %}`{{ st.anchor }}`{% else %}缺口——{{ st.gap }}{% endif %}{{ "，" ~ st.note if st.note }}
{% endfor %}
{% if p.prerequisites %}
- 前提：{{ p.prerequisites | join("；") }}
{% endif %}
{% endfor %}
{% endfor %}
{% endif %}
{% for f in s.findings if f.status == "retracted" %}

- 已撤回：{{ f.fields.title }}——{{ f.retracted.reason }}
{% endfor %}
