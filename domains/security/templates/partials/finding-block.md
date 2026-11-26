{% for f in s.findings if f.status == "validated" %}

#### [证实 · {{ f.fields.severity }} · 置信度 {{ f.fields.confidence }}] {{ f.id }} {{ f.fields.title }}
- 类别：{{ f.kind }}；语义锚：`{{ f.identity.anchor }}`{{ "（实例：" ~ f.identity.instance ~ "）" if f.identity.instance }}
- 陈述：{{ f.fields.claim }}
- 前提：{{ f.fields.preconditions }}
- 影响：{{ f.fields.impact }}
- 严重度理由：{{ f.fields.severityRationale }}
- 方法：{{ f.fields.method }}
- 证据：
{% for a in f.fields.evidence %}
  - `{{ a }}`
{% endfor %}
- 反证考虑：{{ f.fields.counterevidence }}
- 身份定位：`{{ f.anchors | join("`、`") }}`
{% if f.paths %}
- 完整路径：
{% for p in f.paths %}
{% for st in p.steps %}
  - {{ st.role }}：{% if st.anchor %}`{{ st.anchor }}`{% else %}缺口——{{ st.gap }}{% endif %}{{ "（" ~ st.note ~ "）" if st.note }}
{% endfor %}
{% if p.prerequisites %}
  - 前提：{{ p.prerequisites | join("；") }}
{% endif %}
{% endfor %}
{% else %}
- 路径：无
{% endif %}
{% if f.warnings %}
- ⚠ 警告：{{ f.warnings | map(attribute="message") | join("；") }}
{% endif %}
{% endfor %}
{% for f in s.findings if f.status == "refuted" %}

#### [推翻] {{ f.id }} {{ f.fields.title }}
- 陈述：{{ f.fields.claim }}
- 推翻理由：{{ f.fields.rationale }}
- 定位：`{{ f.anchors | join("`、`") }}`
{% endfor %}
{% for f in s.findings if f.status == "deferred" %}

#### [搁置] {{ f.id }} {{ f.fields.title }}
- 陈述：{{ f.fields.claim }}
- 搁置理由：{{ f.fields.rationale }}
- 定位：`{{ f.anchors | join("`、`") }}`
{% endfor %}
{% for f in s.findings if f.status == "candidate" %}

#### [未结论] {{ f.id }} {{ f.fields.title }}
- 陈述：{{ f.fields.claim }}
{% endfor %}
{% for f in s.findings if f.status == "retracted" %}

#### [已撤回] {{ f.id }} {{ f.fields.title }}
- 撤回理由：{{ f.retracted.reason }}
{% endfor %}
