# 制作域包

域包是一个域的全部约定：规则（`pack.json`）、模板集（`templates/`）、方法（`method/`）。内置三个域：`security`、`wiki`、`investigation`，用 `project-scanner packs list` 看。内置的域直接在 `scan_start` 里按名字用，不需要导出。

## 放在哪

| 层 | 位置 | 能做什么 |
|---|---|---|
| 内嵌 | 随可执行文件 | — |
| 用户级 | `$PSCAN_HOME/packs/<域名>/`（默认 `~/.local/share/project-scanner/packs/`） | 新域；或替换内置的域，做自己的变体 |
| 项目级 | `<项目根>/.project-scanner/domains/<域名>/` | 新域；或在已有域上追加和收紧 |

三层按文件合并，项目级最优先。目录名就是域名：小写字母、数字、连字符。`.project-scanner/` 本身不算待覆盖的文件。

- 用户级是你自己机器上的文件，可以替换任何文件，包括 `pack.json`。
- 项目级随仓库来。扫别人的仓库时它是外部内容，所以叠在已有域上时只准：
  - 收紧片段：加必填、收窄枚举、调小上限；
  - 追加不接纳记录的导航页输出（`select: sections`）；
  - 新增或替换方法文件。

  不能改 `pack.json`，不能替换已有的输出和 partial，不能放宽片段。要改规则就新起一个域名。
- 扫描开始时生效的域包固定成快照，之后改文件不影响进行中的扫描；报告开头写明这次叠了哪几层。

## 目录结构

```
<域名>/
  pack.json
  templates/
    fragments/     片段：声明每类记录在每个阶段的字段
    outputs/       输出：渲染成什么文件
    partials/      被 outputs 引用的子模板，没有前置元数据
  method/
    planning.md  discovery.md  validation.md  path.md   按阶段给 agent 的做法
    其他参考文件
```

## pack.json

```json
{
  "domain": "infra-config",
  "description": "查基础设施配置里的风险：权限过宽、未加密、来源不明的镜像。",
  "sectionKinds": ["module", "baseline"],
  "findingKinds": ["misconfig", "secret", "dependency"],
  "pathRequiredKinds": ["misconfig"],
  "pathRoles": ["source", "transform", "control", "sink"],
  "originRoles": ["source"],
  "outcomeRoles": ["sink"],
  "verdicts": ["validated", "refuted", "deferred"],
  "severities": ["critical", "high", "medium", "low", "informational"],
  "confidences": ["high", "medium", "low"],
  "dispositions": ["no_issue", "not_applicable", "deferred"],
  "defaultExclusions": ["node_modules/", "dist/", ".git/"],
  "policyFile": "SECURITY.md",
  "anchorNaming": "小写字母数字、连字符分段，描述失效之处的作用",
  "method": {
    "planning": "method/planning.md",
    "discovery": "method/discovery.md",
    "validation": "method/validation.md",
    "analyze_full_path": "method/path.md"
  },
  "references": ["method/severity-rubric.md"]
}
```

| 字段 | 要求 |
|---|---|
| `domain` | 和目录名一致 |
| `description` | 一句话。接入时、`packs list`、域不认识时的报错里都是它 |
| `sectionKinds` `findingKinds` `pathRoles` | 非空 |
| `pathRequiredKinds` | ⊆ `findingKinds`；这些类别证实后必须有路径 |
| `originRoles` `outcomeRoles` | ⊆ `pathRoles`；路径至少一个起点、一个结果 |
| `verdicts` | 非空；要和 validation 片段里 `verdict` 字段的枚举一致 |
| `severities` `confidences` | 取值列表，给 agent 和片段的枚举用；没有评级就留空 |
| `dispositions` | 非空 |
| `method` | 阶段 → 文件。四个阶段都可以给，文件必须存在。`scan_context` 在对应阶段把正文原样返回 |
| `references` | 按需读的文件，必须存在。`scan_context` 只列文件名，`view=pack` 读全文 |
| `policyFile` | 相对项目根的路径；没有就 `null` |
| `defaultExclusions` `anchorNaming` | 默认排除的目录前缀；语义锚的命名约定，原样给 agent |

## 模板集

- **片段要齐**：`section` 的 planning、`exclusion` 的 planning、`finding` 的 discovery 和 validation、`path` 的 analyze_full_path，各至少一份通用片段（`select` 不带 `kind`）。缺了，那一类记录就没有可绑定的片段，永远带警告。`finding` 在 analyze_full_path 的片段可选：要给证实后的断言再加字段（比如影响面）就写，没有就只由路径片段管那个阶段。
- **字段声明**：`type` 只能是 text / enum / anchors / refs / list / count；另有 `values`、`required`、`when`、`min`、`max`。声明是数据，不是 Jinja。
- **输出**：`select: all` 和 `select: section` 合计恰好一份——`all` 一份接纳全部记录，`section` 每个板块一份。多了记录会被接纳两次，少了没人接纳，封存都会拒绝。`select: sections` 是导航页，不接纳记录，可以有任意份。`key` 是 Jinja，只能用 `section.label`、`section.id`、`scan.id`、`scan.domain` 和 `slug` 过滤器。
- **写法**：块标签独占一行；行内的可选片段写 `{{ 表达式 if 条件 }}`，不要在行尾放 `{% endif %}`；列表拼一行用 `join`；断言块之间要空行，就让每个块以一个空行开头。上下文的键是 camelCase。
- 模板引擎是 minijinja：没有自动转义，只有 `slug` 过滤器，有步数和输出大小上限。结尾的换行由内核规范化。

## 方法文件

Markdown，写给 agent 看的"这个域在这个阶段具体怎么做"：怎么切板块、提什么样的断言、什么算证实、路径怎么跟。放行条件不用写，内核会给。每份控制在一屏左右；评级标准、检查清单这类细则放进 `references`。

## 做一个新域

1. 起个域名，选一个最接近的内置域导出当起点：`project-scanner packs export security /tmp/my-domain`。
2. 改 `pack.json`（`domain`、`description`、类别、角色）、`templates/`、`method/`。内置域的内容只是起点，不必保留。
3. 验证：`project-scanner packs check /tmp/my-domain`。它检查规则字段的包含关系、每个模板文件能否解析、必需的片段是否齐、接纳记录的输出是否恰好一份，并用一份合成状态把每个输出试渲染一遍。有问题时逐条列出，退出码 1；域名不合法这类用法错误是 2。
4. 放到位置：只给这个项目用，放 `<项目根>/.project-scanner/domains/<域名>/`（可以提交）；所有项目都用，放 `$PSCAN_HOME/packs/<域名>/`。
5. `project-scanner packs list` 里能看到就可以用了：`scan_start { domain: "<域名>", prompt: 用户原话 }`。

在已有域上做项目级收紧时，目录里只放要改的文件。验证用 `project-scanner packs check <目录> --base security`：它按"只能收紧"的规则合并后再查。

## 命令

| 命令 | 作用 |
|---|---|
| `project-scanner packs list [--root 目录]` | 域名、来源（内嵌 / 用户 / 项目）、描述 |
| `project-scanner packs show <域> [--root 目录]` | 合并后的 `pack.json`、每个文件来自哪一层、方法文件 |
| `project-scanner packs check <目录> [--base 域]` | 验证一个域包目录，或它作为某个域的覆盖 |
| `project-scanner packs export <域> <目录> [--root 目录]` | 导出合并后的域包，作为新域的起点 |
| `project-scanner skills export <目录>` | 导出本技能 |