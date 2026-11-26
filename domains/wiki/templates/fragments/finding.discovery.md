---
type: fragment
select: { record: finding, stage: discovery }
fields:
  title: { type: text, required: true, max: 120 }
  claim: { type: text, required: true, max: 2000 }
  anchors: { type: anchors, required: true, min: 1 }
---
- {{ finding.fields.title }}：{{ finding.fields.claim }}
