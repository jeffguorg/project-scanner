---
type: output
key: REPORT.md
select: all
---
# 安全扫描报告

- 目标：`{{ scan.root }}`{{ "（" ~ scan.remote ~ "）" if scan.remote }}
- 修订：`{{ scan.revision }}`{{ "，含未提交的改动，内容摘要 `" ~ scan.snapshotDigest ~ "`" if scan.dirty }}
- 域：{{ scan.domain }}；扫描：{{ scan.id }}；生成于 {{ scan.generatedAt }}
{% if scan.packOverlays %}
- 域包：规则来自 {{ scan.packSource }}；叠加了 {% for o in scan.packOverlays %}{{ o.level }}（{{ o.files | join("、") }}）{% if not loop.last %}，{% endif %}{% endfor %}

{% endif %}
- 模板集摘要：`{{ scan.templatesDigest }}`

## 范围

用户要求（原话）：

{% for r in userRequests %}
- {{ r.text }}
  （{{ r.at }}，{{ r.source }}）
{% endfor %}

显式排除：

{% for e in exclusions %}
- `{{ e.path }}`：{{ e.reason }}
{% else %}
- 无
{% endfor %}

默认排除：{{ "`" ~ defaultExclusions | join("`、`") ~ "`" if defaultExclusions else "无" }}

{% if policy %}
## 安全策略

{{ policy | trim }}

{% endif %}
## 概览

- 板块状态：
{% for st, n in counts.sections | items %}
  - {{ st }}：{{ n }}
{% endfor %}
- 断言：证实 {{ counts.findings["validated"] | default(0) }}，推翻 {{ counts.findings["refuted"] | default(0) }}，搁置 {{ counts.findings["deferred"] | default(0) }}，撤回 {{ counts.retractedFindings }}
- 需要路径的证实断言：{{ counts.pathsRequired }}，其中路径完整 {{ counts.pathsComplete }}
- 过期定位：{{ stale | length }} 条

## 各板块

{% for s in sections %}
### {{ s.label }}（{{ s.id }}，{{ s.kind }}）

状态：{{ s.status }}。范围：{{ "`" ~ s.paths | join("`、`") ~ "`" if s.paths else "—" }}

待查问题：

{% for q in s.questions %}
- {{ q }}
{% endfor %}
{% if s.disposition %}

处置：{{ s.disposition }}——{{ s.dispositionReason }}
{% endif %}
{% include "partials/finding-block.md" %}

{% endfor %}
## 过期的定位

{% for x in stale %}
- {{ x.findingId }}：
{% for a in x.anchors %}
  - {{ a }}
{% endfor %}
{% else %}
- 无
{% endfor %}

## 已撤回的断言

{% for f in retracted %}
- {{ f.id }} {{ f.fields.title }}（{{ f.sectionId }}）：{{ f.retracted.reason }}
{% else %}
- 无
{% endfor %}
