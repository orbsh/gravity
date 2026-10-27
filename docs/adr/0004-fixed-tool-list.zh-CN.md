# 0004 — 工具清单是固定的、写死的、分组的

> **语言：** [English](0004-fixed-tool-list.md)（主文档） · [中文](0004-fixed-tool-list.zh-CN.md)

**状态：** Accepted（2026-10-26）。同日落地：turn 在每一轮推理都发送固定的 `tools` 数组，
四个名字**全部**端到端接上——`recall_memory`、`commit_memory`、`resolve_skills`（k10r
ADR-0015 的 skill 节点与它的门），以及 `invoke_skill`（把一步走到它的 code 行、对到服务该
语言的 mount、在节点上把脚本跑起来）。真实栈由 prism 的 `tests/turn_invoke_skill.rs` 证明。
**日期：** 2026-10-26

**本记录中撤回的名字：** `memory_search` / `memory_recall` / `memory_top` /
`memory_timeline` / `skill_search` / `skill_run` 出现于本记录的初稿，现已撤回。工具面本来
就有名字——k10r 的 `docs/SEAM.md` §2 把它命名为 `recall_memory`、`commit_memory`、
`resolve_skills`、`invoke_skill`——而同一个操作有第二套词汇，正是架构记录存在的意义要拦下
的那种漂移。

## 背景

Phase 2 落地的是工具调用**循环**——guest 解析 `tool_calls`、分发、把结果喂回、重开
provider——但从来没有任何东西告诉模型有哪些工具。provider 请求里没有 `tools` 数组，而分发
的 target 取自 turn 的 args（`tool_target`，默认 `effector`），模型给的名字被直接当作入口
点。于是这条循环活在一个从未被描述过的面上：真实模型调用不出它没被告知的工具，唯一走过这条
循环的流量，是测试里那个不经提示就吐 `tool_calls` 的 stub。

但工具面本身并不是没设计过——k10r 的 `docs/SEAM.md` §2 逐个操作列了它，带着各自的门和状态。
缺的是关于它**形态**的裁定：这些工具怎么到达模型。

桌上有三种形态。

- **动态：每轮检索 top-k skill，逐个注入成工具定义。** 模型的可见能力变成检索质量的函数；
  没进候选的 skill 等于不存在；工具块每轮都变，也就意味着被缓存的 prefix 每轮都变。
- **固定：一小份写死的工具清单，由它们去够到基底。** 模型永远看到同一组能力；skill 是通过
  这些工具**够到**的（先 `resolve_skills`，再 `invoke_skill`），而不是被摊平进工具清单。
- **运行模式：预设几个 profile，k10r 只是其中一种基底。** 同一个二进制，参数化到不同的
  memory / skill 后端上。

## 裁决

**RULING——gravity 是最上层的应用、纯胶水，所以它的工具清单固定、写死、分组，并沿用 k10r
工具面已有的名字。**

1. **每一组的基底都在上游。** memory 是 k10r 的（图谱门、记忆门、投影）；skill 是 k10r 的
   行加上执行面（ADR-0001 的 mount）。gravity 提供的是组合和一段 prompt，它自己没有基底。
   一份必须**被检索**的能力清单，是一个 gravity 并不拥有的基底带来的后果——基底自己会建
   索引，而够到它正是 `resolve_skills` / `recall_memory` 的职责。
2. **清单写死，分组：memory、skill、task，名字用工具面已有的那一套。** 不造第二套词汇：
   `docs/SEAM.md` §2（k10r）就是登记簿，本记录采用它。

   | 工具 | 参数 | 做什么 | 今天 |
   |---|---|---|---|
   | `recall_memory` | `{query}` | `POST /memories/{memory_scope}/recall`——landing 加它所在的 community，由 k10r 渲染，读取预算也是 k10r 自己的 | 已入清单（gravity `crates/booth/src/tools.rs`） |
   | `commit_memory` | `{facts:[…]}` | `POST /memories/{memory_scope}`——图谱的 ingest | 已入清单 |
   | `resolve_skills` | `{query}`、`{limit}` | **可选 skill 清单**——模型自己完不成时去查的东西；k10r 按意图答候选 | 已入清单（2026-10-26）：k10r ADR-0015 落地了 skill 节点与 `POST /skills/{memory_scope}/resolve`——skill 与事实同图，所以图的 scope 就地址到它 |
   | `invoke_skill` | `{skill_id, step?, arguments?}` | 读 skill、选一步、`GET /code/{id}` 取脚本，然后受治理的调用 `invoke(mount_type, session_key, "execute", {"code": …, "args": …})` 在 effector 节点 mount 上跑它（SEAM.md §3） | 已入清单（2026-10-26）：**一次调用一步**，顺序归模型（见 §3） |

   task 组（`task_open` / `task_done` / `task_list`）已声明但**不入清单**：它的写面还没有
   路由，读侧骑在会话投影上（SEAM.md §2 的「Task progress」；ADR-0003 §3、PLAN Phase
   2.6）。基底办不到的工具是对模型撒谎，而调用过它的模型会连带不再信任整张清单。
3. **流程就是工具面描述的那一条：先 resolve，再 invoke。** 当请求无法用 prompt 和 head 答
   出来时，模型调 `resolve_skills`；候选作为工具结果回到对话里，**带上 steps**（k10r
   ADR-0015 §4——要选就得看见这个 skill 会让你做什么，而清单里没有第五个工具去问）；模型
   挑一个，调 `invoke_skill`——它按 id 取回 code 行，把脚本作为参数交给 mount。任何 skill
   都不会变成它自己的一份工具定义，这条流程里也没有哪一步是模型得去猜的检索。

   **一次调用走一步。** `invoke_skill` 只跑 skill 的其中一步，**顺序是模型的**：它看见了
   各步，它把下一步需要的东西放进 `arguments`，它决定要不要继续。排序、重试、把一步的结果
   喂给下一步，仍是 Phase 5 的事——同一个工具里长出第二个「invoke 的含义」，会把这份控制权
   藏起来。多步 skill 的长期形态不是 N 次调用，而是**一份自己编排各步的脚本**；这一点已在
   ADR-0005 裁决（体是已有语言写的代码，步之间的边从体推导），它改的是 skill 的 `steps` 装
   什么，不是这条流程。

   **入参是一个 JSON map，永远不是 argv。** 一次 mount 调用只带一份 JSON 文档，每一层都保持
   这个形状：gravity 发 `{"code": …, "args": {...}}`，载体把它框成 stdin 上的
   `{"handler": "execute", "args": …}`，runner 再把同一份具名 map 交给脚本（两个 runner 都是：
   `TOOLRUNNER_ARGS` 就是那份 map 的 JSON 字符串，脚本在 stdout 回一份 JSON 文档）。它不会
   展开成 `--flag value`，也不该加：脚本不是一条命令行，而是以数据形式投递的一个体，没有
   shell 语法可展开；argv 是无类型的，那正好会废掉入参刚得到的类型校验；而引号规则会变成每个
   runner 各实现一遍的契约。想要命令行手感的脚本，自己解析自己那份 map。

   这份校验有**两处**声明面，而且是同一种语言——这是有意的：工具面（这份清单，发给 provider）
   的 `parameters` 是 JSON Schema，约束的是**模型**的调用；code 行的 `parameters` 也是 JSON
   Schema，约束的是**脚本**的 `args` map。一套词汇、两个主语；走法所执行的子集
   （`required`、`properties.*.type`、`additionalProperties: false`）本就是这两处都在写的
   子集，所以两边都不用再学一种方言。已知边界：`args` 是经环境变量到达脚本的，超过内核单变量
   上限的载荷是 runner 的事，不是 schema 的事。
4. **不要运行模式。** 模式系统是把应用参数化到它并不拥有的基底上；它真正拥有的基底是
   k10r 和 aura，说一次就够。换一套 memory 平面是另一个应用，不是这个应用上的一个开关——
   会长出 profile 的胶水已经不是胶水。
5. **部署事实走 turn 的 args，永不进清单。** `store_url`、`endpoint`、`model`、
   `key_env` 已经在这么做；scope 加入其中（`memory_scope`）。k10r 的 namespace 编号是
   k10r 自己的声明——把它编译进 gravity，会让 k10r 一次重编号变成静默地读别人的图谱，而
   失败的样子是「答了个空」，不是报错。缺 scope 时，受影响的工具带着缺失事实的名字失败，
   不取默认值。
6. **每个条目声明自己的执行性质——静态、逐工具。** `fast`（进程内、幂等的读）对
   `human`（触到外部世界、花人的注意力或改状态）。这是 Phase 2.5 冷路径读的那个字段，也是
   `tool_invoke_count` 回写挂靠的字段（Phase 5）：`recall_memory` 与 `resolve_skills` 是
   `fast`；`commit_memory` 与 `invoke_skill` 是 `human`。
7. **turn 开头的投影仍是主读取。** head 到手时已经组装并渲染完毕；这些工具回答的是 head
   答不了的——某次具体查找、近似面做不了的枚举。它们不是投影的替代品，也不许变成每轮轮询：
   一个在每个步骤前都先 `recall_memory` 的 turn，已经把一份组装好的视图换成了 N 次往返。
8. **清单按门生长，不按热情生长。** 一个条目在它的基底能应答时才发出去；上面的名字现在就
   定死，免得以后改名。具体说：`recall_memory` 与 `commit_memory` 写好就出去了，
   `resolve_skills` 则在它的门落地的同一天跟上（k10r ADR-0015——当初因零客户端撤掉的
   bring-up 池读，现在有了客户端）。剩下的是 `invoke_skill`：它需要「走完一个被解析出来的
   skill」——`steps[].code_id` → `GET /code/{id}` → 该语言的 mount（Phase 4.6）。

## 考虑并否决

- **top-k skill 注入。** 它把模型的世界模型变成近似检索器的函数：同一个问题在两次运行里可能
  看到不同的能力，新建的 skill 在它的 embedding 收敛前不可见，而工具块（也就是 prompt
  prefix）每轮都变却什么也没换来——真要跑起来模型仍然得再发一次调用。
- **每个 skill 生成一份工具定义、开机时生成。** 清单会随图谱增长，每加一条都要重新部署或每轮
  重建，而且模型上下文会被这场对话永远用不到的 skill 填满。
- **给这些工具另造一套词汇**（`memory_search`、`skill_run` 之类）。那正是本记录的初稿，已
  撤回：工具面的名字在 k10r 的 SEAM 登记簿里，而一个操作两个名字，会把之后每个读者的注意力
  一分为二。
- **运行模式 / 基底 profile。** 上面已否：那是一个配置面，在替另一个产品站位。
- **让模型挑 memory namespace。** 它是部署事实，且有一个写侧的邻居（`commit_memory` 会写
  事实）；一个能选读哪里的模型，也能选写错哪里。
- **现在就把 task 工具声明出来、答「暂不可用」。** 一个永远失败的工具会教会模型「失败是
  常态」，而这正是工具循环最赔不起的习惯。

## 后果

- **能力清单与 prompt 是同一个 prefix。** 固定的 `tools` 数组意味着请求里稳定那一段不在
  turn 之间移动；k10r 的 head 与 summary 排在它后面，而这两样本来就逐 turn 稳定。
- **skill 家族的 k10r 一侧已付**（2026-10-26）：门回来了，schema 也有了——事实图里 kind 为
  `skill` 的节点，四部分按归属拆开（前置 / 步骤 / 后验是 prop，JSON 文本；经验是它自己的
  关联事实），前面站着 `POST /skills/{scope}` / `GET /skills/{scope}/{id}` /
  `POST /skills/{scope}/resolve`（k10r ADR-0015）。gravity 还欠的是 `invoke_skill`：从候选的
  `steps[].code_id` 走到 code 行，再走到该语言的 mount。
- **工具结果是模型的，不是消费者的。** turn 把每个结果喂回循环（assistant 的调用 + 一条
  `tool` 消息，OpenAI 的线上形状），并留在这一轮的 transcript 里；它**不上流**。prism 的
  turn 面只承载 `item.delta` 文本，别的一律拒绝（"envelope item carries no delta
  string"），所以一个 tool_result 条目会在泵那里把整轮打死——这是 smoke 跑出来的，不是单元
  测试。给 UI 用的「正在跑 `commit_memory`…」进度帧，是 prism 要加的协议，不是 gravity 绕过
  别人的面自己发明的帧。
- **PLAN Phase 5** 在 `invoke_skill` 上实现 pull-through 与权重回写：skill 行就是计数器挂靠
  的注册信息，计数是加性数据（ADR-0003 §5），拓扑不动。
- **PLAN Phase 2.5** 拿到它的声明点：条目的 `nature` 是静态元数据，于是「注册时声明为冷」
  不需要一个运行期注册表。
- **PLAN Phase 4.6 已付**（2026-10-26）：它等的 `run` 调用**就是** `invoke_skill`，它等的
  「语言 → mount」匹配也不是一条裁决而是一个**部署事实**——prism 读它自己的 manifest
  （`PRISM_MOUNTS` 本来就声明了两个轴），把 `mounts` 放进 turn 的 args，于是 gravity 拿
  skill 声明的语言去对这份部署实际提供的 mount，对不上就把话说出来（ADR-0001 §7：没有
  mount 的语言不能调）。没有语言**默认值**可裁：语言由 code 行声明，没声明就是模型读到的一个
  错误值。
- **已裁决——多步 skill 的体是代码，不是 DSL**（ADR-0005，2026-10-26）：组合用已有语言写
  （steel 或 python），步之间的边**从体推导**而不是手工维护在一份 JSON 里，本项目任何地方都
  不授权语法、步骤词汇或 JSON 组合格式。组合的体**是一步**，于是 §3 的「一次调用一步」不再是
  限制，而不是被绕开。语言问题逐 skill 定（看 code 行的 `language`），只剩一处待验证：steel
  必须证明能求值投递进来的字符串，它的体才能骑 mount（ADR-0005 §4）。
- **已付（2026-10-26）：入参在节点被要求跑任何东西之前，先对 code 行的 `parameters` 校验。**
  这个字段是 store 从不解析的 TEXT（k10r ADR-0012 §2——形状归生产方），所以 gravity 只校验
  生产方**声明**了的东西，用的是 JSON Schema 的一个子集（`required`、`properties.*.type`、
  `additionalProperties: false`，`type` 可写一个名字或一列名字）。什么都没声明的文档——`{}`、
  空串、为人类写的散文——不构成约束，放行就是**这个意思**：什么都没校验，因为什么都没声明。
  在那里发明一条拒绝，会让「用描述代替 schema」的行全废，而模型对此无能为力。校验失败绝不
  到达节点，会点名出错的键或类型，并把契约原文交回去——模型因此一轮就能改对，而不是从三层
  之下学到形状。
- **开放项：** 图谱其它入口（`top`、`timeline`、`chain`、`anchored`——都已作为门落地）该不该
  变成工具，还是保持它们本来的样子：给已经知道要找什么的调用方用的门。skill 节点的 schema
  与 namespace 已**闭合**（k10r ADR-0015：ns 36 里 kind `skill`，与事实同一个 scope）。

## 相关记录

ADR-0001（执行面：mount、`carrier`、脚本作为参数）、ADR-0002（客户端形态）、ADR-0003
（skill 执行：代码负责组合、进度是数据、code 按 id 标识）、`docs/PLAN.md` 的 Phase
2.5 / 2.6 / 4.6 / 5、以及 k10r 的 `docs/SEAM.md` §2/§3（工具面、名字、门）与
`docs/MODELING.md`（namespace）。