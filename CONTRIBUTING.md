# 参与贡献

## 先读这一份：定位与边界

> **给独立开发者与一人公司：一个人加一个 Agent，就能管住 N 台机器。**

知微的第一用户是**独立开发者 / 一人公司（OPC）**，明确**不含「小团队」**——后者
的痛点是「谁能看什么 / 谁值班 / 怎么交接」，这些需求会把权限、多用户、审计一个
个带回来，也就抹掉了本项目存在的理由。完整论证见
[docs/POSITIONING.md](./docs/POSITIONING.md)。

**准入判据**（提 issue 之前先问这一句）：

> 这是在帮**一个人盯 N 台机器**，还是在帮**一个团队分工**？

### 明确不做

下面这些**不是「还没做」，是决定不做**。相关 issue 会被直接关闭并指向本节——
这不是不欢迎你，而是单人项目抗过载的方式。

| 不做 | 理由 |
| --- | --- |
| 用户 / 角色 / RBAC / 多租户 / SSO / LDAP | **控制台只有单一 admin token，没有「用户」这一层**。第一用户就是一个人，连角色都不需要 |
| 合规审计（谁看了什么） | 属于组织治理，与 OPC 无关。「agent 做了什么」是另一回事，见下 |
| HA / 集群 / 分布式存储 | 监控服务挂了对 OPC 不算灾难，不值得为它上 Postgres / ClickHouse |
| 图表拖拽 / 大屏 / 仪表盘编辑器 | 认知重的根源，与「不用学一套仪表盘语言」直接冲突 |
| K8s 深度集成 | 第一用户跑的是 docker compose 与 systemd |
| 长周期海量指标存储 | 不做 Prometheus 替代 |
| agent 自主决策 / 自动修复 | 既是安全边界也是定位：永远是人点头才动手 |

「agent 做了什么」这一条**要做**（可被机器消费 + 受控操作 + 可审计），但要补的是
**显式授权模型**——谁在什么条件下允许 agent 触发哪一类操作，而不是放开敏感工具。

## 本地跑起来

前置：Rust 1.75+，Node 18+（只改后端可以不装 Node）。

```sh
./scripts/dev.sh start     # 起 monitor + ops + 自动入网一个本地 node
./scripts/dev.sh status    # 进程与日志
./scripts/dev.sh stop
```

控制台在 <http://127.0.0.1:8443/>，admin token 见 `data/admin.token`。
改前端要 `cd ui && npm install && npm run build`；`scripts/dev.sh` 会自动托管
`ui/dist`。完整说明见 [README](./README.md)。

## 提交之前：这些必须过

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

cd ui && npm run build      # 内含语言包一致性检查 + tsc
```

四项全绿是硬要求，CI 会卡。改动涉及 Dockerfile 时另跑
`./scripts/check-dockerfile-crates.sh`（workspace 成员与镜像 COPY 列表必须一致，
曾经因为漏一个 crate 导致镜像构建失败而本地全绿）。

## 提交信息

Conventional Commits，**标题用中文**，一行说清做了什么：

```
feat(nodes): 节点列表分页/排序/过滤 + 节点详情改版
fix(config): ZHIWEI_PLAIN_HTTP 接受 1/true/yes/on 等写法
docs(positioning): 定位为「一个人 + 一个 Agent 管住 N 台机器」
```

常用 scope：`auth` `api` `ui` `node` `ops` `storage` `alerts` `certs` `services`
`logs` `docker` `deploy` `docs` `config`。正文里值得写清楚的是**为什么**和
**怎么验证的**，尤其是踩过的坑。

## 代码约定

**Rust**：新增数据库结构改 `crates/storage/src/migrations.rs`，编号递增、靠
`schema_version` 做幂等判断，不要改已有迁移。

**前端**：遵守 Trilium「前端约束规范」——只用 `brand-*` / `surface-*` / `ink-*`
令牌（禁止硬编码色值），图标只用 Lucide SVG（禁止 emoji），所有文案走 `t()`
并保持中英语言包键数一致，动效不超过 300ms。语言包不平衡时 `npm run build`
会直接失败。

**测试**：新增行为请附可复现的验证方式。这个项目更看重「真的跑过一遍」——
PR 描述里写清跑了什么命令、看到什么输出，比单测覆盖率数字有用得多。

## PR 流程

1. 一个 PR 一件事，小而聚焦。
2. 描述里写：解决什么、怎么验证的（命令 + 实际输出，有截图更好）、有哪些已知限制。
3. CI 绿了再请人看。

## Issue 怎么写

说清三件事：你跑的是什么环境（OS / 部署方式 / 版本）、期望发生什么、实际发生
什么。日志与报错原文请直接贴，不要转述。

## 语言

中文、英文都可以，按你顺手的来。

## License

Apache-2.0。提交贡献即表示你同意以该协议授权。
