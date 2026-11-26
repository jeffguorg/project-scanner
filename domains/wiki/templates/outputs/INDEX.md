---
type: output
key: INDEX.md
select: sections
---
# 项目文档：{{ scan.root }}

项目文档。生成于 {{ scan.generatedAt }}，修订 `{{ scan.revision }}`，域 {{ scan.domain }}。
{% if scan.packOverlays %}
- 域包：规则来自 {{ scan.packSource }}；叠加了 {% for o in scan.packOverlays %}{{ o.level }}（{{ o.files | join("、") }}）{% if not loop.last %}，{% endif %}{% endfor %}

{% endif %}

用户要求（原话）：

{% for r in userRequests %}
- {{ r.text }}
{% endfor %}

## 板块

{% for s in sections %}
- [{{ s.label }}]({{ s.label | slug }}.md)——{{ s.kind }}；{{ s.findingCounts["validated"] | default(0) }} 条描述
{% endfor %}

显式排除：

{% for e in exclusions %}
- `{{ e.path }}`：{{ e.reason }}
{% else %}
- 无
{% endfor %}
