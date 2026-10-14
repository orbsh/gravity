# 0001 — ToolRunner：执行面就是节点上的一个 mount

> **语言：** [English](0001-toolrunner-execution-surface.md)（主文档） · [中文](0001-toolrunner-execution-surface.zh-CN.md)

**状态：** Accepted（2026-10-14）。还没有任何实现落地；本记录固定的是各相位排队
所依据的形态。
**日期：** 2026-10-14

## 背景

Gravity 是 turn 执行器：取会话投影 → LLM 调用及其工具调用循环 → append delta。
Phase 2 已把循环落地。留下两个问题，第二个自带产品层面的截止期。

**1. 一个工具能被调用之前，必须先存在什么？** aura 按**类型**路由调用：
`engine.call(target: InstanceId, handler, args)`，target = 类型名 + 实例键
（`aura/crates/engine/src/lib.rs`）。远程节点只能通过一个已注册的类型触达，其 body 是
`Body::RemoteProbe { node_alias, language, source, encoding }`
（`aura/crates/realm/src/instance.rs` 的分发臂）。这个类型是 aura 为节点规定的绑定
记录——"type-affinity records ARE the binding surface … the type definition IS the
binding record; no new table needed"（`aura/docs/PLAN.md` 4.10）——而今天没有任何
生产代码注册它，只有 aura 自己的测试在构造
（`aura/crates/engine/tests/remote_effector.rs`）。

**2. 涌现的 skill 会不会变成类型？** 没有一种设计能活下来。skill 是 k10r 图谱里的
子图（"高工具指数的子图"），而涌现回路的契约是零陈旧：同一次调用可能按上下文加载
不同经验，同一个 skill 调用一次之后增加了一条经验就变成新版本。而类型注册是一个
部署动作，带的是部署语义——自省（抛一个一次性子进程）、定义行、存储 ns、路由，以及
**重注册即热替换：该类型的路由先被 drop，它的所有实例被 evict**
（`aura/crates/realm/src/registry.rs`）。把 skill（或它的某个版本）绑到这只时钟上，
等于把涌现回路卡在部署上，而不是数据上。

协议线今天已经提供了什么（逐条对代码核过，不是推断）：

- 远程帧本来就分两半携带：`ToolCall { call_id, kind, stream, session, entry,
  language, encoding, args, code }`。`args` 原样透传（`args: job.args.clone()`），
  `entry` 在 effector 侧不被任何东西解析——"This is NOT a lookup key: the Effector
  resolves nothing against it (no registration, no validation)"
  （`effector/crates/protocol/src/lib.rs`）。
- 代码是内容寻址的：effector 按需取回、按帧里断言的 `sha256` 校验、按哈希缓存
  （aura ADR-0027；`effector/crates/runtime/src/remote.rs`）。
- 节点没有入站可达性：它拨出到控制面，连接由 realm 持有、以节点别名索引
  （`effector/crates/runtime/src/remote.rs`；prism 的 `/effector/<alias>` 挂载
  → `realm.effectors`）。
- 节点的拨出连接是它唯一的门，而只有 realm 这一侧持有它——因此**任何到达节点的
  字节都走控制路径**。
- 嵌入式载具在结构上没有沙箱：`spawn_session` 构造 steel/python 会话时不传沙箱策略，
  只有进程载具（induction/exec）在 bwrap 下 spawn
  （`effector/crates/runtime/src/carrier/session.rs`；`sandbox.rs` 直接写着
  "in-process carriers (steel/python)"）。场内摊位则是硬编码
  `SandboxPolicy::None` 被 spawn 的（`aura/crates/realm/src/instance.rs`），而且
  aura 没有"按类型声明沙箱"的面。
- 驻留会话只在冷启动时 load 源码：同一个 session key 拿不同的源码文本过来，跑的还是
  旧代码（`effector/crates/runtime/src/carrier/session.rs`）。

## 裁决

**RULING — 执行发生在节点上、经由一个固定的 runner；skill 是数据。**

1. **ToolRunner 就是执行形态。** 节点 mount 是一个 `RemoteProbe` 摊位类型，其 source
   是一份**固定的部署件**：一个 runner（解释器或 dispatcher），它把操作的代码与参数
   当作调用参数收下并执行。mount 的代码永不随 skill 或版本变化；一切动态的部分都是
   数据。
2. **skill 永不成为类型。** 涌现的 skill 留在 k10r；它的脚本由 gravity 取出，作为
   调用参数携带。没有类型、没有 ns、没有路由、没有定义行、不随 skill 版本热替换——
   也不需要为它做内容寻址的代码交付：脚本是 KB 级，随调用以参数形态走，这根本不属于
   协议意义上的"代码交付"。
3. **每台机器都跑一个节点，控制面那台也包括。** 不存在特权本地路径，gravity 里也不
   存在进程内执行面：本地执行就是对本机节点 mount 的一次调用（loopback）。因此
   gravity 不背任何执行面依赖——effector-runtime 不进它的依赖树。
4. **runner 是部署件（booth）。** 固定代码，随节点安装，在节点清单里声明；它是基础
   设施，不是图谱公民——不被搜索、不被经验命名、不参与权重回写。
5. **记忆与检索类工具不是 ToolRunner 调用。** k10r 的 HTTP 面是数据访问而不是代码
   执行；gravity 继续直连（krystallizer ADR-0007 修订版：HTTP 是唯一的跨进程形态）。
6. **session key 仍然由调用方决定。** `session` 是不透明的驻留身份，由 gravity 选
   （会话、用户，或两者）。它不需要带版本分量——因为 runner 的源码固定，那条
   "只有冷启动 load"的坑咬不到这里。

7. **节点的语言面是声明的、很小的，且永不由数据选定。** 必须安装的只有 nushell 与
   python（steel 编译进节点二进制，无需安装）。nu 是系统操作语言，python 是业务逻辑
   语言；不存在 shell 载具，也不退化成 bash——系统级操作一律路由到 nu 的 mount，因为
   bash 那条路会把 nu 内建命令本就替你消掉的引号、调用、可移植性麻烦重新引回来。
   强制是结构性的、不是劝阻性的：没有 mount 的语言根本调不到，而没有任何数据路径能铸
   出 mount。（nu 脚本仍可能去调外部二进制——那是沙箱策略的事，即节点的 bwrap 政策
   暴露了什么，不是路由的事。）一个操作的语言是 **skill 的属性**（脚本是数据，数据带
   语言），由 gravity 路由到对应 mount；LLM 永不在调用时选载具。精确一点：清单里的
   `language` 指的是**载具**，而操作的语言按约定骑在 **mount 名**上（mount 跑的就是它
   runner 跑的那门语言——`runner_nu` / `runner_py`）；把它升为清单的一等字段是仍开项。
8. **每语言一份 runner；需要边界的地方就是进程——而在进程内求值决定谁能常驻。** 两种
   runner 形态：一次性 `exec` 载具（argv 规格、请求 JSON 走 stdin、stdout 整体读出——
   没有帧、没有驻留、没有 ctx 桥），够跑无状态的系统操作，也是最容易部署的一种；以及
   驻留的 `induction` 载具（说帧协议的 guest，带驻留与 ctx 桥），下发的代码会在
   guest 自己的进程里跑。**已核实：第二种形态在 nu 里不存在**——nu 无法在进程内对一段
   运行时字符串求值（`source` 要求路径是解析期常量，且 nu 0.116 没有 `eval` 命令），
   所以 nu guest 只能 shell 出一个子进程，而子进程永远看不到 guest 的 ctx 作用域。
   进程内求值正是常驻的前提：python 可以——guest 进程里的 `exec` 让下发脚本跑在自己的
   作用域里，而 guest 的 ctx 辅助函数在那可见——nu 不行。于是 nu 是一次性系统语言
   （`deploy/runners/runner_nu.nu`），python 今天是一次性
   （`deploy/runners/runner_py.py`），并且是常驻形态的候选语言（等一份 python guest
   存在；那份 guest 目前是尚不存在的部署件）。嵌入式 python 载具（pyo3，进程内 CPython
   跑在节点自己的进程里）依旧**不是** runner 形态：它不容纳进程边界。

## 已考虑并否决

- **把 effector 集成进 gravity（进程内执行面）。** 它换来"单件发布"的内聚，代价是
  沙箱：嵌入式载具在结构上不容纳进程边界，LLM 生成的代码会在 gravity 的进程里跑。
  它还会造出第二个驻留归属（第二张会话表、第二份驱逐策略、第二套沙箱配置），与场域
  那一份并立——正是裁决 3 要消灭的分叉。发布内聚的目标改用打包解决：想要自足安装
  时，场域与它的节点一起发。
- **按调用传内容寻址的代码引用，或把 inline 形态加回来。** 这两条都在回答"字节住在
  哪"，而这个问题只在"调用携带**代码**"时才存在。有了 runner，变化的部分是参数，
  问题不成立：ADR-0027 原样（runner 是定义——blob + ref，正是今天那一类），不需要
  协议改动，也不需要 errata。
- **本地用场内 runner 摊位，不铺每机器一节点。** 省掉一台机器一个常驻进程。否决理由：
  场内 spawn 硬编码 `SandboxPolicy::None`，也没有可用的按类型沙箱声明面；而且
  steel/python 的场内摊位是嵌入式的——根本没有进程边界。它还会让"同一个调用在本地
  与远程行为不同"。
- **本地用 effector 库执行、远程走 mount。** 两个驻留归属、两套沙箱策略、两个测试
  面，而调用点只有一个。调用点统一了，语义没有。

## 后果

- **gravity：** 无执行面依赖；工具表固定且极小（skill 检索 + run）；调用点一处——
  `invoke(mount, key, entry, args{code, args})`；脚本从 k10r 的 HTTP 面来，以参数
  形态走。
- **k10r：** 脚本的家，且 gravity 是它唯一的客户；节点永不与它说话——字节路径是
  gravity → 控制路径 → 节点。
- **prism：** 多出一件工作——mount 注册面。按 (别名 × 语言) 注册一个
  `RemoteProbe` 类型，source 是那份 runner 部署件。`RemoteProbe` 定义不持久化，所以
  这是每次启动的幂等注册；拨入帧 `Frame::Register { node_alias, carriers }` 已经提供
  了别名与载具集，可作触发源。
- **aura：** 零改动。用的全是既有面（`engine.register`、`Body::RemoteProbe`）。
- **effector：** 零改动。runner 是定义，skill 脚本是参数；帧词汇、编解码、代码引用
  全部照旧。
- **部署：** 每台机器（含控制面）跑一个节点；每语言一份 runner 部署件（或一台
  dispatcher）随它安装；节点清单声明载具、沙箱策略与容量。"部署在哪，就能操作哪"
  （aura ADR-0015）变成字面事实：一台机器的能力就是它的清单。
- **驻留：** 一张会话表、一份驱逐策略、一套沙箱配置——场域与节点的，永远不是
  gravity 的。
- **被排除：** gravity 单件跑且具备执行能力；任何本地特权执行路径；按 skill 或按版本
  的注册；协议层的按调用代码形态。
- **在此定下：** runner 绑定的 entry 是 `execute`——effector 自己的兜底约定（源码没绑
  的名字会落回 `execute`，只要代码定义了它）让它成为唯一稳妥的选择，测试钉住。
  每语言一道 mount，分派在调用方：gravity 按 skill 声明的语言路由到对应 mount。
  节点侧的 dispatcher 进程被否决——它会把载具运行时的会话、池化、沙箱那套在部署件里
  重做一遍，只换来省掉几个静态注册。清单的撰写与审核归**部署层**（ADR-0015：节点身份
  不是用户身份）；**谁能用哪个节点**归**应用层**（gravity 的 realm/实例键 + prism 的
  auth 门）。
- **仍开：** prism 今天把 `Frame::Register.carriers` 丢掉了
  （`crates/prism/src/effectors.rs` 里的 `_(carriers)`），所以"语言 ↔ 节点"这条事实
  还没有落点——要么在节点清单里声明，要么把载具集接进注册与路由。

## 相关记录

aura ADR-0025/0031（tier-1 远程执行——本记录所依托的 mount）、aura ADR-0027（内容
寻址的代码交付；未被触碰）、aura ADR-0015（节点身份与审批面）、aura ADR-0044
（无进程内摊位）、krystallizer ADR-0007（HTTP 是唯一跨进程形态）、`docs/PLAN.md`
（排队：Phase 4.6、Phase 6）。