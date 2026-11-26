---
type: fragment
select: { record: finding, stage: analyze_full_path }
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
  impact: { type: text, when: { verdict: validated }, required: true, max: 500 }
---
（路径阶段的片段只加约束，不单独渲染；见 REPORT.md）
