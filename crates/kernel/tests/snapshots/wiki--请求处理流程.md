# 请求处理流程

板块 s-08，类别 module。范围：`src/app.ts`

待说明的问题：

- 一个请求怎么走完
## 端到端流程

### 请求从入口到响应（f-006）

请求经中间件链到达处理器，再由渲染器输出。

- source：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`
- transform：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`，中间件
- sink：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`
- 前提：普通用户可登录
