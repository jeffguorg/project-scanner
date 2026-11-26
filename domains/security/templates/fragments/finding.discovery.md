---
type: fragment
select: { record: finding, stage: discovery }
fields:
  title: { type: text, required: true, max: 120 }
  claim: { type: text, required: true, max: 2000 }
  anchors: { type: anchors, required: true, min: 1 }
---
#### {{ finding.id }} {{ finding.fields.title }}
- 陈述：{{ finding.fields.claim }}
- 定位：
{% for a in finding.anchors %}
  - `{{ a }}`
{% endfor %}
