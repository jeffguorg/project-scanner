---
type: output
key: "{{ section.label | slug }}.md"
select: section
---
{% for s in sections %}
# {{ s.label }}

板块 {{ s.id }}，类别 {{ s.kind }}。范围：{{ "`" ~ s.paths | join("`、`") ~ "`" if s.paths else "—" }}

待说明的问题：

{% for q in s.questions %}
- {{ q }}
{% endfor %}
{% if s.disposition %}

处置：{{ s.disposition }}——{{ s.dispositionReason }}
{% endif %}
{% include "partials/finding-block.md" %}
{% if s.warnings %}

⚠ 警告：{{ s.warnings | map(attribute="message") | join("；") }}
{% endif %}
{% endfor %}
