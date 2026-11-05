# 0007 — 模型配置是 gravity 自己的，并且存放在场域里

> **语言：** [English](0007-model-configuration-is-gravitys.md)（主） · [中文](0007-model-configuration-is-gravitys.zh-CN.md)

**状态：** 已接受（2026-10-30）。这是一条关于归属性与存放位置的裁决；池化的选择规则仍开放。
**日期：** 2026-10-30

## 背景

一次 turn 需要五件不属于任何请求的事实：记忆库的 URL、LLM 的 endpoint、模型名、持有密钥的环境变量名，以及 k10r 记忆门所寻址的 scope。今天这五件全部由 prism 组装（`crates/prism/src/turns.rs` 的 `TurnConfig::from_env`），作为调用参数交给摊位。

放在错误的仓库里。Prism 是通用网关：它认证一条连接、解析页面发来的东西、把一个事件放上平面。哪个模型作答、在哪个 endpoint、用哪个凭据名，不是网关的事实——那是执行器自己的配置；网关持有它，就把「换一个模型」变成「重新部署网关」，并让一个本应什么都不知道的组件里长出了另一个组件的业务词汇。

所以这些事实移到 gravity。但事实需要位置，而 gravity 没有：turn guest 没有声明任何存储。机制其实已经在——`Engine::register` 会内省 script 摊位、把定义（含 schema）持久化并在途中分配该类型的 ns，realm 再从这份持久化副本构建出它的 `ctx.store` plan——只是这棵树里还没人用过：该形状由 steel 载体的 fixture 走过，而没有任何 induction guest 声明过 `storage` 块。Rust body 的摊位则永远不可能有（编译期 handler 不可持久化），这也正是测试里的 collector 会打印 `no storage ns` 的原因。场域才是它该在的地方，而场域就是 aura：realm 的嵌入式存储，guest 通过 ctx 上已有的 remote store 臂（`PutDocument` / `GetDocument` / `PutFields` / `GetFields` / `Scan` / `DeleteDocument`）读写，行数据活过每一个进程，且在一次 turn 内部只有一次本地读的距离。

密钥保管规则不变，且它约束形状：密钥的**值**永不离开进程环境。一行可以写环境变量的名字（`key_env`）；永远不可以写密钥本身。

## 决定

**裁决 —— 模型配置归 gravity 所有，以行数据存放在场域里（aura，经 remote store），由 gravity 提供的接口管理。Prism 只传页面知道的东西。**

1. **Prism 不持有任何模型词汇。** 没有 endpoint、没有模型名、没有 `key_env`、没有 memory scope：prism 的 turn 实现整体删除，页面的提交只带它知道的东西——它所在的会话、文本、它要的输出形状。只有执行器读的部署事实，归执行器。

2. **这些事实是 gravity 自己 collection 里的行，由 turn 在开始时读取。** 一份 settings 文档（store URL、memory scope、默认模型），加一张按 alias 寻址的模型表。这次读是本地且廉价的——存储是引擎的，不是一次网络跳转——并且每轮 turn 都读，因为 turn 正是选择生效的地方。

3. **每一行模型记录都是一份完整配方，所以池化就是同一张表。** `{alias, endpoint, model, key_env, weight, enabled, affinity}`：一行就是简单部署（alias `default`，weight 1），多行就是池化部署。亲和键是**会话**：一个会话留在同一个模型上，因为 prompt cache 就是以会话为单位建立的。池子如何在合格行之间挑（权重、轮询、平手）是后续模式，**不在本记录中固定**；它的行在这里固定，因为一旦池子活在别处，它就立刻变成第二个真相来源。

4. **管理面是一个视图，和这个应用显示的一切一样。** gravity 收到 `ui:gravity-model`，发出帧展开一块模型管理布局；里面按钮发出普通事件，gravity 执行，页面再向它要一次屏。这是这套栈的通用交互模式，不是 gravity 的发明——prism `docs/MODELING.md` 写的就是这个模型：场馆推布局，页面的上行下行都**不带前缀**（prism 在上行加 `ui:`、下行剥离，ADR-0017），每个动作都是事件，AI 也能驱动同一批事件、不需要第二个面。它刻意**不是**一个 admin HTTP 动词、一个配置文件或一个环境变量：一个家，而且是用户看得见的家。存储本身也是如此——同一场域上的多个应用各有自己的 ns，于是应用声明一次 collection，就不再需要考虑行存放在哪里。

   屏是**一棵根**，不是往一个列表里反复追加的行。`ui:gravity-model` 经订阅到达（`on_as("ui:gravity-model", "device", "screen")`，auth 与其余相同为 `true`），用 `create` 回答——和 shell 在 `ui:connect` 上画的是同一棵根——每张表一份表单（`models`、`settings`），挂在它 body case 的 children 上（`frame_root(forms)`）。问两次就替换一次根，屏始终只有一张：配置屏是一个部署的整张脸，不是流；两张固定表单也没有行 id 可供 `append` 打键（给它们加上 id 只会让第二次回答变成渲染器拒收的重复行，ADR-0005 的规矩）。调用方不指定槽位——屏长什么样由 booth 自己说了算，而点击本来就带不了槽位。

   表单用 FIELDS 说明自己写哪一行，而不是用目的地：`collection` 与 `id` 以绑定为表单字段的 `text` 节点随提交一起走（`{kind: "field", field: "collection", default: "models"}`），因为 fluxen 的表单收集每个 `field` 绑定的子节点并发送 `{field: {data, payload}}`，而 text 节点没有可输入的控件，所以这对值会跟着提交走、也不会被重打。隐藏字段更好，但模型里没有——为一个屏幕去给 fluxen 加一种节点是反方向。每张表单提交到**裸名** `gravity-model.save`：页面上行不带标记，标记由场馆在入口加 `ui:`。

   名字背后的动作是 `ui:gravity-model.save`，与 turn 一起声明（`gravity_booth::register` 里的 `on_as`，guest 里的处理器 `save`），并由 prism 读取的同一份 auth 声明把守（`{"ui:gravity-model.save": true}`）。它的载荷是一行——`{collection: "settings"|"models", id, doc}`——写进本类型的 ns；它也接受表单的形状 `{field: {data, payload}}`，在一处解包，早于校验，数字也在这里归一（输入框的文本是字符串，`number` 输入是浮点，而声明的宽度是 8 字节——`"37"` 编码不了）。部署不能配置的集合、或缺少 turn 要读的键的文档，按名拒绝。两个回答都是 EVENT——`ui:gravity-model.saved`，以及列出缺键的 `ui:gravity-model.error`——因为经事件到达的处理器没有可返回的对象（ADR-0009 §4）。

   未配置的部署照样能画：屏幕用普通 `get_document` 读行，不用 `read_settings`（后者按名拒绝，因为 TURN 不能在半个部署上跑）；空行给空表单——而空表单正是"还能被配置"的唯一状态。

   拒绝要说两遍，因为它有两个读者。程序读事件；人读屏幕，而一个非数组载荷被注入到没人绑定的槽里等于看不见——页面上「被拒」和「什么都没发生」长得一样。所以屏带一块通知区：一个浮动区里装着绑定固定槽 `notice` 的 rack，在表单之上，空着时不占版面；拒绝把同一句 detail 作为一行追加进去。帧里不带任何生命周期：一行停多久、怎么进场、是否轮换、什么让它停，都是渲染器的 `view`（fluxen ADR 0024）——依「部署只说消息，怎么显示归 UI」这条裁定。线上因此只多了一个事件名 `ui:gravity-model.notice` 和一个槽名 `notice`。

5. **全新部署没有行，而这必须是一个有名字的错误。** turn 大声失败，指出缺失的文档与键，而不是回落到环境变量。回落就是换个名字的第二个家。

6. **保管规则不动。** 行里写 `key_env`（一个名字）；值活在 guest 进程的环境里，由 provider guest 按该名字读取。任何配置面——存储的、论证过的、被提供的——都不携带密钥。

## 已考虑并否决

- **prism 在路由事件时把事实注入信封**（本记录所取代的做法）。它让 prism 的代码行数更少、但性质是错的：携带执行器 endpoint 的网关，会在模型改变时被重新部署，而那个注入点是没人会去找的地方。
- **只用环境变量。** 没有面向用户的接口、没有池化、没有不重新部署就能改的覆盖，也没有除了读进程环境之外回答「现在哪个模型在作答」的办法。
- **gravity 部署里的一个配置文件。** 文件与存储在任何一方被编辑的第一次就不一致；而存储已经是摊位作为公民自己读写的机制。
- **以 k10r 为家。** k10r 是记忆基底——会话日志、事实图谱、任务。模型配方是执行器的部署状态：它在一轮 turn 有任何记忆可查之前就要被读到，它不是按相似度召回的，把它放到 HTTP 跳转之后，会让 turn 自身的起点依赖上它随后本就要调用的服务。
- **让页面选模型。** 页面是渲染器；模型选择是一项能力与成本决定。如果用户需要按会话选择，那应当是一行数据加一条亲和规则，而不是上行帧里的一个字段。

## 后果

- **Prism 的 `turns.rs` 随其余 turn 代码一起消失**（`TurnConfig`、槽位计数器、`run_turn`），layout 的 `turn.submit` 变成平面上一个普通事件。prism 保留的是场馆基础设施：它自己的节点清单（`mounts`）与平面本身。
- **turn 的 args 缩到页面知道的范围**——会话、文本、输出形状。部署事实不再逐调用旅行，于是调用方也不可能漏传一件事实、让同一个输入得到不同的行为。
- **gravity 的摊位在 `.auth(...)` 旁边声明 `.storage(...)`，注册工作就这些——2026-10-30 已证**：induction 载体的 schema 块第一次就变成了 realm 里的 plan（`.storage()` 收的是 **collections 映射**，因为组装好的 schema 已经把它嵌在 `storage` 之下；再包一层就是"这个类型什么也没声明"）。`Engine::register` 内省 guest、持久化 schema（并在途中分配该类型的 ns），realm 从这份持久化副本构建 plan，因此它活过重启。
- **一个类型只有一个 ns，所以类型内的 collection 必须带键判别位**（实测，aura ADR-0026 的形状）：`store_exec` 对每个 collection 都用 `plan.ns` 建集合、只编码声明的键字段，于是两个用相同 `id` 的 collection 写的是**同一行**——后写的静默覆盖先写的，读回来是另一个表的名字下的这套字段。gravity 的表因此把键定成 `[kind, id]`（`settings` = 1，`models` = 2）。将来加第三张表必须取新的 kind；在任何摊位里想加第二个 collection，都应该先假设这个坑还在。
- **池化变成一次表编辑，而不是一次部署。** 权重与亲和性是数据；选择规则是代码，且归 gravity。
- **CLI 走数据模式，两种模式由调用方选择。** `protocol: fluxen` 画图（帧，槽位由调用方声明）；delta 消费者自己声明 chunk 以哪个事件说话，而场馆下行会剥掉 `ui:` 标记（prism ADR-0017），所以 CLI 看到的就是它本来就在说的裸名字。两种模式不是两套实现：一次 turn，两种呈现（ADR-0009）。
- **turn 寻址设备，永远不是 channel。** 传输面只知道设备与用户（prism ADR-0018）；channel 是应用自己的业务，而这个应用没有——没有群聊，两个用户之间甚至不直接对话。turn 回答提出请求的那台设备；摊位自己发出的载荷不带 `sender`，因为场馆会把发送者从它自己事件的扇出里排除。
- **端到端验过：** `turn_booth::the_screen_draws_the_forms_that_write_the_rows`——空表单 → 两帧提交保存事件 → 一次表单形状的点击写入两行、含数字 → 屏幕重画出示刚存的值 → turn 在其上跑通；`scripts/smoke-venue.sh` 在**真场馆**上请求屏幕，先断言两张表单与它们的目的地，再从进程外配置部署；以及一次真浏览器（系统 chromium 走 CDP）：按 `manage` → 两张预填好的表单 → 改别名 → 按 save → 再问一次 → 还是两张，值是新值，没有第三对。
- **开放项：** 池化的选择规则（权重、平手）。

## 相关记录

ADR-0001（执行面——mount 与作为参数的脚本）、ADR-0002（客户端形态）、ADR-0004（固定工具表）、ADR-0009（一次 turn，两种渲染）、gravity PLAN Phase 4.1 的 `NO PULL FACE` 记录（turn 是画图的 event handler，这正是 turn 的事实不能再随某个 puller 组装的请求旅行的原因）。prism `docs/MODELING.md`（模型：应用本身就是视图，管理面包括在内——本记录是它的第一个落地实例）。prism ADR-0017/0018（线协议；设备，不是频道）。aura ADR-0026（类型的存储 plan 来自它已持久化的声明）与 aura ADR-0031/0034（摊位的代码自己决定访问方式；ctx 是跨摊位面）。一次 turn 拿到 URL 之后从记忆库读什么，见 k10r 的 `docs/SEAM.md`。
