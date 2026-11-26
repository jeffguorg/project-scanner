# 问题调研报告

- 目标：`/srv/app`，修订 `0123456789abcdef0123456789abcdef01234567`，含未提交的改动
- 扫描：aaaaaaaaaaaa；生成于 2026-01-01T01:00:00.000Z

## 问题

用户要求（原话）：

- 检查这个项目中可能存在并可利用的 bug。
  （2026-01-01T00:00:00.000Z，prompt）
- 尤其注意跨租户越权。
  （2026-01-01T00:10:00.000Z，append）
- 先不管前端。
  （2026-01-01T00:20:00.000Z，answer）

## 结论概览

- 假设：成立 2，否定 1，搁置 1
- 板块状态：
  - abolished：1
  - excluded：1
  - follow_up：1
  - investigating：1
  - merged：1
  - no_issue：1
  - reported：2
- 显式排除：
  - `examples/`（合成示例，不随产品发布）
  - `vendor/x/`（上游生成的客户端代码）

## 各板块

### 认证（s-01，attack-surface）

状态：reported。范围：`src/auth`

待查问题：

- 登录渲染是否转义用户输入
- 登出是否清理会话

### [成立 · rce · 置信度 medium] f-001 登录页渲染未转义
- 假设：用户名直接进模板上下文，模板若允许表达式则可执行
- 理由：name 未转义进入 render
- 证据：
  - `0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`
- 反证考虑：管理后台同样渲染但需管理员
- 身份定位：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`、`0123456789abcdef0123456789abcdef01234567:src/auth/login.ts:block_start(export function logout)`
- 因果路径：
  - source：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`
  - transform：缺口——模板引擎内部处理未读到（框架黑盒）
  - sink：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`
  - 前提：普通用户可登录
- ⚠ 警告：定位过期：第二个定位所在文件已改

### [已撤回] f-004 登录名拼 SQL
- 撤回理由：实际是参数化查询

### 上传（s-02，attack-surface）

状态：excluded。范围：`src/upload`

待查问题：

- 上传文件名是否净化

### [否定] f-002 上传路径穿越
- 假设：文件名未净化
- 否定理由：有 basename 处理
- 定位：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`

### 依赖与配置（s-03，baseline）

状态：no_issue。范围：`package.json`

待查问题：

- 依赖有没有已知漏洞

处置：no_issue——没有第三方依赖

### 结账（s-06，attack-surface）

状态：investigating。范围：`src/checkout`

待查问题：

- 优惠券叠加能否为负

### [未结论] f-003 优惠券代码进了模板
- 假设：coupon.code 被当模板渲染

### 配置（s-07，baseline）

状态：follow_up。范围：`config/`

待查问题：

- 调试开关是否默认关闭

### [搁置] f-005 调试端点默认开启
- 假设：DEBUG 路由没有环境判断
- 搁置理由：要确认部署配置
- 定位：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`

### 请求处理流程（s-08，module）

状态：reported。范围：`src/app.ts`

待查问题：

- 一个请求怎么走完

### [成立 · flow · 置信度 high] f-006 请求从入口到响应
- 假设：请求经中间件链到达处理器，再由渲染器输出。
- 理由：沿调用链读完
- 证据：
  - `0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`
- 反证考虑：
- 身份定位：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`
- 因果路径：
  - source：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`
  - transform：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`（中间件）
  - sink：`0123456789abcdef0123456789abcdef01234567+dirty:src/auth/login.ts:block_start(res.render('login', { name }))`
  - 前提：普通用户可登录

## 过期的定位

- f-001：
  - 定位过期：第二个定位所在文件已改

## 已撤回的断言

- f-004 登录名拼 SQL（s-01）：实际是参数化查询
