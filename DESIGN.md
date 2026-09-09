# alpaca-trading-system-rust — 设计 & 决策

> 这份文档是我们讨论的落点：目标、已定的架构决策、模块划分、落地顺序、以及还没定的问题。
> 代码开写前先在这里对齐，改主意就改这份。

## 1. 目标

一个基于 Alpaca 的行情/交易学习系统，分两条线：

1. **近期可交付**：每日行情摘要（daily report），生成一个自包含 HTML。
2. **远期方向**：agentic trading —— 实时决策 + 下单 + 决策/订单审计。

顺带明确的**学习目标**：Scylla（分布式宽列建模）+ tokio（异步）。
> 诚实备注：以本项目的数据量，Scylla 是**学习驱动**而非需求驱动——单机 Postgres/QuestDB 完全够用。
> 我们是"演一个还没有的规模"来学工具，这很正当，但不自欺成"需要"。

## 2. 架构决策（已定）

- **热路径永不同步查 DB**。DB 一个来回的尾延迟不可预测，会毁掉实时决策。
- **大历史是用来"造状态"的，不是决策时读的**：离线/盘前把历史压成内存里的紧凑状态
  （均线、波动率、参考位、模型参数——几个 float），盘中用实时流**增量更新**（ring buffer +
  Welford/EMA），从不回读历史。
- **DB 只站在两头**：盘前 warm-up 读一次；决策/订单/成交**异步写出**（不挡路）。
- **活状态 vs 历史，严格分开**：
  - 活的订单状态/持仓（agent 决策依据）→ **强一致**（Postgres 或内存+WAL），**绝不放 Scylla**
    （最终一致会导致重复下单/仓位算错）。
  - 不可变的历史/审计/事件流 → **Scylla**（append-only、按 symbol/account 分区、按时间聚簇）。
  - **铁律**：决策读强一致状态库，历史库只用于事后分析/审计。

## 3. 数据库分工（已定）

| 库 | 角色 | 数据 | 为什么 |
|---|---|---|---|
| **QuestDB** | 服务/时序层 | 日线 bar、算好的信号 | 专为行情造、写快、说 Postgres wire 协议 → Rust 用 `tokio-postgres`/`sqlx` 零门槛；报表读它 |
| **Scylla** | 原始/审计层 | crypto 原始 tick 流、订单/决策审计事件 | 写多、宽列时序模型地道、async driver 顺带练 tokio；只做不可变追加 |

> Redis/内存等"活状态"缓存以后按需再加，先不引。

## 4. 数据源（Alpaca，已核对的约束）

- **免费档 WebSocket 可用**，但：只有 **IEX 源**（全市场零头）、**1 条数据连接**、订阅数有上限。
- **全市场 SIP 实时流 = 付费**（月费几十刀量级；价格以官网为准）。
- **Crypto：数据免费 + 24/7**，端点 `v1beta3/crypto/us/...`。
  → **练 write-heavy / tokio 的最佳免费来源**（不打烊、量比 IEX 好）。
- 想压 Scylla 写吞吐极限：**合成 tick 更狠**（免费 Alpaca 喂不饱）。

## 5. 报表内容（daily report，已定）

一个 HTML，四块。其中 2/3/4 配**内联 SVG 折线（sparkline）**——纯字符串生成，零 JS、零依赖、可离线 test。

1. **板块热力图** —— 已有（当日 vs 昨收，涨绿跌红）。
2. **Daily movers** —— 全体按当日涨跌% 排序，取涨榜/跌榜头部。
3. **Watchlist 表现** —— 订阅股列表 + 各自折线。
4. **Buy/Short 推荐** —— 复合信号过滤出的强买/强卖。

> 关键洞察：2/3/4 是**同一份 per-symbol 数据的不同筛选/排序**，一套数据 + 四个视图，不是四套抓数逻辑。

## 6. 复合信号（已有，教学用，非投资建议）

三个子信号加权成 `[-1,1]` 分数 → 五档评级：

- 趋势：收盘 vs SMA20（±8% 满格）· 权重 0.40
- 动量：SMA5 vs SMA20 金叉/死叉（±5% 满格）· 权重 0.35
- RSI(14)：超买/超卖，逆势刹车 · 权重 0.25
- 分档：≥0.5 强买 · ≥0.2 买 · >-0.2 持有 · >-0.5 卖 · else 强卖

## 7. 模块划分（已定：Leptos，core + web）

前端用 **Leptos**（Rust→WASM）。原生抓数依赖（reqwest/DB driver）不能编到 wasm，
所以拆两块：

- `crates/core` —— **纯逻辑，无依赖**：领域类型 + 信号 + 数据模型。因为无依赖，
  **native 和 wasm 都能编**，其 test 也能跑（`cargo test -p alpaca-core`）。
  原生抓数（reqwest/DB）以后放**单独的 crate**，不塞进 core。
- `crates/web`  —— **wasm**：Leptos 前端，依赖 core，`trunk` 构建。**自成一个 workspace**，
  免得根 `cargo build` 去 host 编 wasm-only 代码而失败。

```
alpaca-trading-system-rust/
├── Cargo.toml              root workspace（只含 core）
├── DESIGN.md
└── crates/
    ├── core/               [pure, native+wasm] 共享域
    │   └── src/
    │       ├── lib.rs
    │       ├── signals.rs   指标 + 复合信号（已移植+test）
    │       └── data.rs      Sector/Timeframe/pct_change + mock（真代码+test）
    └── web/                [wasm] Leptos 前端（自成 workspace，依赖 core）
        ├── Cargo.toml      leptos(csr) + console_error_panic_hook + alpaca-core
        ├── index.html      trunk 入口
        ├── Trunk.toml
        └── src/
            ├── main.rs      wasm 入口：panic hook + mount App（你写 mount）
            ├── app.rs       根组件：slider + heatmap（spec 注释，你写）
            └── components/
                ├── heatmap.rs           板块热力图组件（spec，你写）
                └── timeframe_slider.rs  可拖动 timeframe 条（spec，你写）
```

**MVP（当前这片）**：板块热力图 + 可拖动 timeframe 条 —— 拖动→按 timeframe
重算各板块涨跌%→重染色。先用 `mock_sectors()` 把 Leptos/交互跑通；真数据
（core 抓 Alpaca → JSON）之后接。movers/watchlist/推荐 之后再加。

**工具链**：`rustup target add wasm32-unknown-unknown` + `cargo install trunk`
（都已装）。开发：`cd crates/web && trunk serve --open`。

## 8. 落地顺序（已定）

1. **报表先用 QuestDB 跑通**：Docker 起 QuestDB → 建时序表 → 拉日线+算信号落库 → 出 `report.html`。先有能看的成品。
2. **Scylla 作为并行"写多"学习线**：Docker 起单节点 → 定 tick 时序表 → tokio async driver 接 crypto 流灌原始 tick。不阻塞报表。
3. 远期：engine（实时决策 + 下单 + 审计）。

## 9. 还没定的（open questions）

- Watchlist 具体是哪几只 ticker？
- 折线窗口：60 天还是 30 天？
- Movers 取几只（涨 N + 跌 N）？
- 推荐只显示强买+强卖，还是五档全显？
- 项目位置：现放在 `untitled/alpaca-trading-system-rust`，要不要单独 repo？
- 旧 `alpaca` 里的 stream/history/heatmap：移植后是删是留？
