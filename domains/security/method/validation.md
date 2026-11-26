# validation：证实或推翻

- 逐条 `verdict.attach`。对着源码把利用链走一遍，每一步要有定位。
- 证实（validated）：`method` 写怎么确认的（读代码、写复现、对照框架默认行为）；`evidence` 是链上的定位；`counterevidence` 写为什么不成立的顾虑；`preconditions` 写成立需要什么；`severity` 和 `confidence` 分开评级，理由各写一句。
- 推翻（refuted）：`rationale` 写实际是什么（"有 max(subtotal, 0) 兜底"）。推翻的断言保留，报告里可见，说明查过。
- 没查完的写 deferred 加理由，不要写成没问题。缺证据不等于推翻，也不等于降级：写明什么事实会改变结论。
- 评级标准见 `method/severity-rubric.md`。
- 发现新问题先 `scan_progress { reopen: [板块] }`，再在那个板块新建。
