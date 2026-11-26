# discovery：写描述

每条描述是一条可被源码证实或推翻的断言：

- `description`（这段代码做什么）、`invariant`（必须成立的约束）、`convention`（约定）、`flow`（端到端流程）。
- `identity.anchor` 描述这段代码的作用（`checkout-total-calculation`）；增量时靠它认出"同一条"。
- `anchors` 指向定义处。`claim` 用读者能懂的话，不要复述代码；一句一条，不要把整页话塞进一条。
