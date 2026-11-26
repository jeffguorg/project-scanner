---
type: fragment
select: { record: finding, stage: validation }
fields:
  title: { type: text, required: true, max: 120 }
  claim: { type: text, required: true, max: 2000 }
  anchors: { type: anchors, required: true, min: 1 }
  verdict: { type: enum, values: [validated, refuted, deferred], required: true }
  rationale: { type: text, required: true, max: 1000 }
  evidence: { type: anchors, when: { verdict: validated }, required: true, min: 1 }
  confidence: { type: enum, values: [high, medium, low], when: { verdict: validated }, required: true }
---
（验证阶段的片段只加约束；渲染见各板块页面）
