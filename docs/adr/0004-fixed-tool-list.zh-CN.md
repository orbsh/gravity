# 0004 — 工具清单是固定的、写死的、分组的

> **语言：** [English](0004-fixed-tool-list.md)（主文档） · [中文](0004-fixed-tool-list.zh-CN.md)

**状态：** Accepted（2026-10-26）。同日部分落地：turn 在每一轮推理都发送固定的 `tools`
数组，memory 组那两个（`recall_memory`、`commit_memory`）已端到端接上；skill 组仍在等
`resolve_skills` 的门（k10r，PLAN Phase 5）。
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
   | `recall_memory` | `{query}` | `POST /memories/{memory_scope}/recall`——landing 加它所在的 community，由 k10r 渲染，读取预算也是 k10r 自己的 | 门已落；工具是 gravity 要写的 |
   | `commit_memory` | `{facts:[…]}` | `POST /memories/{memory_scope}`——图谱的 ingest | 门已落；工具是 gravity 要写的 |
   | `resolve_skills` | `{query}` | **可选 skill 清单**——模型自己完不成时去查的东西 | **无门**：k10r 在 2026-10-20 因零客户端撤除了 bring-up 的池读；skill 节点是 Phase 5+，而「清单随它一起到，不提前」（SEAM.md §2） |
   | `invoke_skill` | `{name, args}` | 两步，只有第一步是 k10r 的：`GET /code/{id}` 取脚本，然后受治理的调用 `invoke(mount_type, session_key, "execute", {"code": …, "args": …})` 在 effector 节点 mount 上跑它（SEAM.md §3） | code 行门已落；调用是 gravity 的 Phase 4.6 |

   task 组（`task_open` / `task_done` / `task_list`）已声明但**不入清单**：它的写面还没有
   路由，读侧骑在会话投影上（SEAM.md §2 的「Task progress」；ADR-0003 §3、PLAN Phase
   2.6）。基底办不到的工具是对模型撒谎，而调用过它的模型会连带不再信任整张清单。
3. **流程就是工具面描述的那一条：先 resolve，再 invoke。** 当请求无法用 prompt 和 head 答
   出来时，模型调 `resolve_skills`；候选作为工具结果回到对话里；模型挑一个，调
   `invoke_skill`——它按 id 取回 code 行，把脚本作为参数交给 mount。任何 skill 都不会变成
   它自己的一份工具定义，这条流程里也没有哪一步是模型得去猜的检索。
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
   定死，免得以后改名。具体说：`recall_memory` 与 `commit_memory` 写好就可以出去；
   `invoke_skill` 需要 `resolve_skills` 产出的 skill 身份（以及 prism 已经声明过的
   language → mount 匹配）；`resolve_skills` 需要它的门回来——k10r 撤掉的那个读当时没有
   客户端，现在有了。

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
- **k10r 欠一道门**（`resolve_skills`）**和一份 schema**（skill 节点：前置 / 步骤 / 经验 /
  后验，PLAN Phase 5+）。skill 家族在 gravity 这一侧就是 `invoke_skill` 的那次调用。
- **PLAN Phase 5** 在 `invoke_skill` 上实现 pull-through 与权重回写：skill 行就是计数器挂靠
  的注册信息，计数是加性数据（ADR-0003 §5），拓扑不动。
- **PLAN Phase 2.5** 拿到它的声明点：条目的 `nature` 是静态元数据，于是「注册时声明为冷」
  不需要一个运行期注册表。
- **PLAN Phase 4.6** 收窄：它一直在等的 `run` 调用**就是** `invoke_skill`，而「语言 → mount」
  的匹配已经写在 prism 的 `PRISM_MOUNTS` 里（`carrier`、`language`）。它还欠的是：当 skill
  行没声明语言时，语言默认值怎么定。
- **开放项：** skill 节点的 schema 与它所在的 namespace（k10r 的声明）、`invoke_skill`
  传给脚本的入参形状、以及图谱其它入口（`top`、`timeline`、`chain`、`anchored`——都已作为
  门落地）该不该变成工具，还是保持它们本来的样子：给已经知道要找什么的调用方用的门。

## 相关记录

ADR-0001（执行面：mount、`carrier`、脚本作为参数）、ADR-0002（客户端形态）、ADR-0003
（skill 执行：代码负责组合、进度是数据、code 按 id 标识）、`docs/PLAN.md` 的 Phase
2.5 / 2.6 / 4.6 / 5、以及 k10r 的 `docs/SEAM.md` §2/§3（工具面、名字、门）与
`docs/MODELING.md`（namespace）。