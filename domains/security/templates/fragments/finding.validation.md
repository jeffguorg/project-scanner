---
type: fragment
select: { record: finding, stage: validation }
fields:
  title: { type: text, required: true, max: 120 }
  claim: { type: text, required: true, max: 2000 }
  anchors: { type: anchors, required: true, min: 1 }
  verdict: { type: enum, values: [validated, refuted, deferred], required: true }
  rationale: { type: text, required: true, max: 2000 }
  method: { type: text, when: { verdict: validated }, required: true, max: 500 }
  evidence: { type: anchors, when: { verdict: validated }, required: true, min: 1 }
  counterevidence: { type: text, when: { verdict: validated }, required: true, max: 2000 }
  preconditions: { type: text, when: { verdict: validated }, required: true, max: 1000 }
  severity: { type: enum, values: [critical, high, medium, low, informational], when: { verdict: validated }, required: true }
  severityRationale: { type: text, when: { verdict: validated }, required: true, max: 1000 }
  confidence: { type: enum, values: [high, medium, low], when: { verdict: validated }, required: true }
---
#### {{ finding.id }} {{ finding.fields.title }}：{{ finding.fields.verdict }}
- 陈述：{{ finding.fields.claim }}
- 理由：{{ finding.fields.rationale }}
{% if finding.fields.verdict == "validated" %}
- 方法：{{ finding.fields.method }}
- 前提：{{ finding.fields.preconditions }}
- 严重度：{{ finding.fields.severity }}——{{ finding.fields.severityRationale }}
- 置信度：{{ finding.fields.confidence }}
- 证据：
{% for a in finding.fields.evidence %}
  - `{{ a }}`
{% endfor %}
- 反证考虑：{{ finding.fields.counterevidence }}
{% endif %}
