# 认证

板块 s-01，类别 attack-surface。范围：`src/auth`

待说明的问题：

- 登录渲染是否转义用户输入
- 登出是否清理会话
## 描述
- **登录页渲染未转义**（f-001，rce）：用户名直接进模板上下文，模板若允许表达式则可执行
  - 依据：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`
  - 置信度：medium

- 已撤回：登录名拼 SQL——实际是参数化查询
