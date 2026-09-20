# 证书管理模块化设计（CA provider · ACME · 续签流水线）· 设计

> 日期：2026-09-19 · 状态：**设计稿，待本人拍板后再实现**
> 依据：[`POSITIONING.md`](../../POSITIONING.md)（认知轻 / Agent 可调用 / 自托管单二进制）
> 前置已完成：证书路径来源 + 节点侧扫描 + 到期告警（`a3647dc` / `32b732b`）

本文回答一个问题：**「证书到期」告警之后那一步——续签——本项目做到哪一层。**

## 1. 先明确问题边界

续签不是「调一个 API」，而是一条六步链路，每一步都可能单独失败：

```
发现 → 判定（该不该续） → 续签（拿新证书） → 分发（送到 nginx 等消费方）
     → 生效（reload / reload 失败回滚） → 验证（新证书真的在服务、链完整）
```

现在的实现覆盖第 1、2 步（扫描 + 到期告警）。第 3–6 步是本文要定的范围。

## 2. 三条路线

| | A 自研 ACME 客户端 | B 编排既有客户端（acme.sh / certbot / lego） | C 只做验证与分发，续签交给用户 |
| --- | --- | --- | --- |
| 做什么 | 内嵌 instant-acme，自己做账号、订单、挑战、轮询 | 节点上调用已有客户端（<code>acme.sh --renew</code> 等），知微负责触发、观察、验证、分发 | 只做「续签后」的检查与分发 |
| 用户侧成本 | 最低（只需 DNS/HTTP 凭据 + 域名） | 低（需先装客户端；多数自托管用户本来就有） | 中（续签本身用户自己管） |
| 我们的成本 | 高：ACME 状态机 + 挑战 + 各家 DNS API + 账号密钥保管 + 错误处理 | 中：探测客户端存在性、参数拼装、输出解析、幂等与重入 | 低 |
| 风险 | **一次 ACME 边缘行为变化就能让「续签」变成「把线上证书搞没」**；且要长期跟 RFC8555 与各家 DNS API | 客户端本身久经考验，出问题也是它的锅；我们只做编排 | 无新风险 |
| 与定位的关系 | 「轻」的反面：把重活揽进单二进制 | 认知轻：用户不需要学新东西 | 认知轻但价值薄 |

**结论：走 B，并把 A 作为未来的 provider 之一留出接口。**
理由不是「B 省事」，而是**续签是破坏性动作**：它可能在 reload 失败时让站点直接不可用。
把最容易出错的那段交给成熟客户端（它们处理挑战超时、DNS 传播延迟、账号配额的经验比我们多），
知微做自己真正擅长且别人没做的那部分：**跨机器的统一编排、可见性、验证与回滚**。

## 3. 组件划分（四个单元，各自可测）

### 3.1 CA provider trait（运行在节点侧）

```rust
/// 一个「续签执行器」。每个实现自己知道怎么调外部工具、怎么判断成功。
#[async_trait]
pub trait CertRenewer: Send + Sync {
    fn name(&self) -> &str;                       // "acme.sh" / "certbot" / "manual"
    /// 探测本机是否具备该 renewer（二进制存在 + 需要的凭据文件就位）
    async fn probe(&self) -> RenewerStatus;
    /// 执行续签。必须是幂等的：已经续过（未到续签期）应返回 AlreadyFresh 而不是报错。
    async fn renew(&self, req: &RenewRequest) -> Result<RenewOutcome>;
}
```

首个实现是 `AcmeShRenewer`（调 `acme.sh --renew -d <domain> --force`），其次是 `CertbotRenewer`。
`manual` 实现永不续签、只在界面上给出「该续签了」的指引——**下限用户（1 台 VPS + 手动证书）也必须有可用路径**。

`RenewRequest` 只包含：域名、证书路径、renewer 名字、超时。
**DNS 凭据、ACME 账号密钥一律不进 monitor**：它们以文件形式留在节点上（`~/.acme.sh/...`），
知微只传「续哪个域名」。

### 3.2 续签编排器（运行在 monitor 侧）

职责：决定该不该续（期）、派活、记录、退避重试、成功后触发验证。

- 输入：证书来源配置（已有）+ 每个来源的续签策略（新增）：`auto_renew`（开关）、`renew_before_days`（默认 30）、`renewer`（默认自动探测）
- 动作：给目标节点下发 `renew_cert`（走既有 ops 签名 → 节点验签 → 回执通道）
- **退避**：失败后不每分钟重试。1h → 6h → 24h，最多 3 次；之后只保留告警，等人工介入
- **上限**：同一节点同一时刻只允许一条续签命令在飞（节点的 CA 客户端往往有全局锁）

### 3.3 分发与生效（节点侧）

续签完成后，把新证书落到消费方（nginx 目录）+ reload：

| 步骤 | 做法 | 失败处理 |
| --- | --- | --- |
| 拷贝 | 按来源配置的 `deploy_to`（目录 + 文件名模板）复制 fullchain / key | 拷贝失败 → 不改动线上，报错 |
| 备份 | 拷贝前把现有文件另存 `*.bak-<时间戳>` | — |
| reload | `nginx -s reload`（可配置命令） | reload 失败 → **回滚到备份并再 reload 一次**，把「回滚也失败」标成 critical |
| 验证 | 进程内 TLS 握手读回 subject / SAN / 到期时间 | 与预期不符 → 不删备份，报错 |

这也是本项目对「证书管理」的真正差异点：多数工具只负责续，**知微负责确认「续完之后线上真的是新证书」**。

### 3.4 可见性

复用已有的证书页与告警：每条来源多一列「续签」（关闭 / 到期前 N 天自动续 / 上次结果 + 时间），
续签失败进待办与告警（`source=cert.renew`），成功则写审计（`audit_log` 已有）。

## 4. 数据模型（迁移 011，待实现）

```sql
-- 续签策略挂在已有的证书来源上（来源已经是「节点 + 路径」）
ALTER TABLE cert_sources ADD COLUMN renew_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cert_sources ADD COLUMN renew_before_days INTEGER NOT NULL DEFAULT 30;
ALTER TABLE cert_sources ADD COLUMN renewer TEXT NOT NULL DEFAULT '';        -- 空 = 节点侧自动探测
ALTER TABLE cert_sources ADD COLUMN deploy_to TEXT NOT NULL DEFAULT '';      -- 空 = 不部署，只续签
ALTER TABLE cert_sources ADD COLUMN reload_cmd TEXT NOT NULL DEFAULT 'nginx -s reload';

-- 续签历史（比告警更细：每次尝试一行，含输出来源与耗时）
CREATE TABLE cert_renewals (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    domain TEXT NOT NULL,
    renewer TEXT NOT NULL,
    state TEXT NOT NULL,          -- running | ok | failed | rolled_back
    message TEXT NOT NULL DEFAULT '',
    started_at_unix_nano INTEGER NOT NULL,
    finished_at_unix_nano INTEGER
);
```

## 5. 安全边界（不放松的三条）

1. **私钥不出节点**：证书私钥、ACME 账号密钥、DNS API 凭据都留在节点文件系统；monitor 只见「路径 + 域名 + 结果」。
2. **续签是敏感动作**：`renew_cert` 进命令白名单，但**默认关闭**（`renew_enabled=0`），且要求显式开启；手工触发同样走 ops 签名 + 审计。
3. **不引入任意命令执行**：`reload_cmd` 只接受白名单前缀（`nginx -s reload` / `systemctl reload nginx` / `docker exec ... nginx -s reload`），不做通用 shell。
   > 这条与既有 `kill_process` / `container_*` 的白名单策略一致；「不做任意命令执行」是命令通道的底线。

## 6. 分阶段落地（每阶段都能单独交付价值）

| 阶段 | 内容 | 前置 |
| --- | --- | --- |
| S1 | provider 探测 + `renew_cert` 命令 + 手动「立即续签」按钮（不做自动） | 迁移 011 的第一批字段 |
| S2 | 自动续签（阈值触发 + 退避 + 并发约束）+ 续签历史 | S1 稳定 |
| S3 | 分发与 reload（含备份、回滚、验证） | S2，且有真实 nginx 场景可测 |
| S4（可选） | 自研 ACME provider（instant-acme），作为 B 之外的备选 | 前三级都被真实使用过 |

**为什么先手动再自动**：续签的第一次必须有人看着。让用户在真实机器上点一次「立即续签」、
看完整回执与验证结果，比我们默认开启自动续签更负责，也更容易暴露权限与路径问题。

## 7. 明确不做

- 不内置 ACME 账号注册 / 挑战求解（S4 之前）；不做 DNS provider 面板（凭据留在节点）
- 不做证书签发以外的 PKI 能力（不签发内部 CA 终端实体证书、不做 mTLS 证书轮换）
- 不做证书透明日志（CT）监控、OCSP stapling 检查
- 不托管用户私钥，也不提供「把私钥上传到知微」的入口

## 8. 风险

| 风险 | 缓解 |
| --- | --- |
| reload 失败导致站点不可用 | 备份 + 回滚 + 验证三级，任一失败都留痕并进告警 |
| 自动续签触发频率过高触发 CA 配额 | 退避 + 「未到阈值不续」+ 同一节点串行 |
| 用户把 `reload_cmd` 填成危险命令 | 白名单前缀校验（§5 第 3 条） |
| 各发行版 acme.sh / certbot 路径不一 | `probe()` 返回可用性与版本，界面直接显示「本机不可用」而不是到续签时才报错 |
