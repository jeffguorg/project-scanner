# 结账

板块 s-06，类别 attack-surface。范围：`src/checkout`

待说明的问题：

- 优惠券叠加能否为负
## 描述
- 优惠券代码进了模板（f-003）：未结论——coupon.code 被当模板渲染
