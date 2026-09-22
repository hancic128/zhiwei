# 部署指南

知微的 monitor-server（数据平面）可以部署到自建主机，也可以部署到托管平台
（Render / Railway / Northflank 等）。两者的差别只有一个：**TLS 由谁终结**——
自建时由 monitor 自己终结（默认），托管平台上由边缘终结（加 `--plain-http`）。
节点身份由 Ed25519 请求签名承担，不依赖传输层，所以两种形态的安全模型一致。

---

## 1. 本地 / 自建主机

### 直接跑二进制

```sh
cargo build --release --bin zhiwei-monitor
./target/release/zhiwei-monitor --data-dir /var/lib/zhiwei --listen 127.0.0.1:8443
```

首次启动会：

1. 在 `<data-dir>/ca/` 生成自签名 root CA（10 年）——**这是全集群的信任根**
2. 签发 monitor server 证书（90 天，带 SAN：localhost / 127.0.0.1 / ::1）
3. 创建 SQLite 数据库 `<data-dir>/monitor.db`（WAL）
4. 打印一次性 bootstrap token（10 分钟有效）——节点入网用

### Docker

```sh
docker build -t zhiwei-monitor .

docker run -d --name zhiwei-monitor \
  -p 8443:8443 \
  -v zhiwei-data:/var/lib/zhiwei \
  zhiwei-monitor
```

查看 bootstrap token：

```sh
docker logs zhiwei-monitor | grep 'BOOTSTRAP TOKEN'
```

国内网络构建慢时可以走镜像：

```sh
docker build --build-arg CARGO_MIRROR=https://rsproxy.cn/index/ -t zhiwei-monitor .
```

托管平台（边缘终止 TLS）上要以明文 HTTP 监听，把 `PORT` 和 `ZHIWEI_PLAIN_HTTP`
一起注入即可：

```sh
docker run -d --name zhiwei-monitor \
  -e PORT=10000 -e ZHIWEI_PLAIN_HTTP=1 \
  -v zhiwei-data:/var/lib/zhiwei \
  zhiwei-monitor
```

### 架构（Apple Silicon / ARM 机器必读）

若在 Apple Silicon（M 系列）上开发，`docker build` 默认产出 **linux/arm64** 镜像，
而 Render / Railway / Northflank 的运行节点是 **amd64**。直接推上去会起不来，
必须显式指定目标平台：

```sh
docker build --platform linux/amd64 -t zhiwei-monitor .
```

镜像大小参考：约 105 MB（arm64 / 无 apt 依赖）。
镜像内已包含控制台（`ui/dist` 由 node 阶段构建后拷入，`ZHIWEI_UI_DIR=/app/ui/dist`），
启动后直接访问根路径即可看到页面，不需要额外部署前端。

> 控制台凭据（admin token）有三个来源，详见下面的
> [「控制台凭据从哪来」](#控制台凭据从哪来zhiwei_admin_token)。自建且挂了持久卷时，
> 首启会生成一次并打印 `[ADMIN TOKEN] <token>`，之后也可以
> `cat /var/lib/zhiwei/admin.token` 读回。托管平台建议直接用
> `ZHIWEI_ADMIN_TOKEN` 环境变量固定下来——免费层既没有 Shell 也挂不了卷，
> 只靠日志的话每次冷启动都会换一个新 token。

> **改 Dockerfile 时注意**：workspace 的每个成员都必须在依赖桩层被 COPY 到，
> 漏一个会在构建镜像时报 `failed to load manifest for workspace member`——
> 本地 `cargo build` 测不出来（本地整棵树都在）。改完跑
> `scripts/check-dockerfile-crates.sh` 校验。

---

## 2. 配置项

| 来源 | 键 | 说明 |
| --- | --- | --- |
| 环境变量 | `PORT` | PaaS 注入。**没有显式配置 listen 时**，绑定 `0.0.0.0:$PORT` |
| 环境变量 | `ZHIWEI_DATA_DIR` | 数据目录（覆盖配置文件） |
| 环境变量 | `ZHIWEI_LISTEN` | 监听地址（覆盖配置文件与 `PORT`） |
| 环境变量 | `RUST_LOG` | 日志级别，默认 `info,zhiwei=debug` |
| 命令行 | `--config` | 配置文件路径，默认 `config/monitor.toml` |
| 命令行 | `--data-dir` | 同 `ZHIWEI_DATA_DIR` |
| 命令行 | `--listen` | 同 `ZHIWEI_LISTEN` |
| 配置文件 | `data_dir` / `listen` / `server_cert_cn` | 见 `config/monitor.toml.example` |
| 命令行 / 环境变量 | `--plain-http` / `ZHIWEI_PLAIN_HTTP` | 明文 HTTP 监听，TLS 交给前置边缘（托管平台用）；自建不要开 |
| 环境变量 | `ZHIWEI_NODE_BASE_URL` | 节点二进制的自建分发源。设了它，控制台生成的入网命令会自动带 `ZHIWEI_BASE_URL`，节点不再从 GitHub 拉包（国内 / 隔离网络用，见第 9 节） |

优先级：命令行 > 环境变量 > 配置文件 > 默认值。

### 控制台凭据从哪来（`ZHIWEI_ADMIN_TOKEN`）

控制台登录用的 admin token 有三个来源，优先级从高到低：

| 来源 | 何时用 | 重启后 |
| --- | --- | --- |
| `ZHIWEI_ADMIN_TOKEN` 环境变量 | 托管平台（尤其免费层） | **不变**，且不写盘 |
| `<data-dir>/admin.token` 文件 | 自建 / 有持久卷 | 不变 |
| 都没有时随机生成 | 首启 | 无持久卷时会重新生成 |

托管平台的免费层既没有 Shell、也挂不了持久 Disk，所以「首启生成后去
`cat <data-dir>/admin.token`」这条路走不通，而且**不挂盘时每次冷启动都会换
一个新 token**，旧的就登录不上了。把 `ZHIWEI_ADMIN_TOKEN` 设成一个固定值
（或在 Render 面板里用 `generateValue`）就能稳定下来——凭据在部署面板里，
不需要进容器。

日志里仍然会打印 `[ADMIN TOKEN] <token>`，但**只在随机生成时打印**：环境变量
来源不打印（避免明文凭据留在日志系统里），也不写盘。

> **两个 token 环境变量都有 16 字符下限。** 短于这个长度的值会被**拒绝并忽略**
> （打一条 ERROR，然后退回文件 / 随机生成），而不是警告一下就照用——它们都是
> 长期有效、又暴露在公网边缘后面的秘密，短了就是可以被暴力猜解的。建议直接用
> ≥32 字符的随机串。

> 控制台凭据只保护**浏览器读接口**。节点走 Ed25519 请求签名，与该 token 无关，
> 所以换 token 不会影响已入网的节点。

### 入网令牌从哪来（`ZHIWEI_BOOTSTRAP_TOKEN`）

节点首次接入要用一个 bootstrap token。同样有两个来源：

| 来源 | 有效期 | 何时用 |
| --- | --- | --- |
| `ZHIWEI_BOOTSTRAP_TOKEN` 环境变量 | **长期有效**（删掉变量并重启即撤销） | 托管平台，尤其免费层 |
| 启动时随机生成，打印 `[BOOTSTRAP TOKEN]` 到日志 | **10 分钟** | 自建 / 本地开发 |

默认那条路在托管平台上很难用：token 只在**进程启动那一刻**打印到日志里，
10 分钟后就失效，而且**每次重启都换一个新的**。免费层既没有 Shell 去看文件，
又会在闲置时缩容重启，等于每次加节点都要去蹲日志抢一个 10 分钟窗口。

设了 `ZHIWEI_BOOTSTRAP_TOKEN` 之后就**不再生成一次性 token**，加节点随时可做：

```sh
ZHIWEI_MONITOR_URL=https://<你的-app>.onrender.com \
ZHIWEI_BOOTSTRAP_TOKEN=<你设的那个值> \
zhiwei-node --state-dir /var/lib/zhiwei-node
```

> ⚠️ 长期有效的入网令牌等于一把「随便谁拿到都能注册节点」的钥匙。请用足够随机的值
> （**下限 16 字符，建议 ≥32**；短于下限会被拒绝并忽略，或直接用 Render 的
> `generateValue`），并且**本机不要提交进仓库**。
> 一个节点入网后就不再需要它了，所以入网完成后删掉这个环境变量、重启，是更稳的做法。

---

## 3. 持久化（**必须**）

`<data-dir>` 里有两样东西**丢了就麻烦**：

| 路径 | 内容 | 丢失后果 |
| --- | --- | --- |
| `ca/ca.key.pem` | CA 私钥 | **所有已入网节点全部失效，必须重新 enroll** |
| `monitor.db` | 节点清单与 telemetry | 历史数据全丢 |
| `monitor.crt.pem` / `monitor.key.pem` | server 证书 | 可重新签发，节点不受影响 |

因此托管平台上**必须挂持久卷**，并让 `ZHIWEI_DATA_DIR` 指向它：

| 平台 | 持久化方式 | 挂载点 |
| --- | --- | --- |
| Render | Disk（付费） | 例如 `/var/lib/zhiwei` |
| Railway | Volume | 例如 `/var/lib/zhiwei` |
| Northflank | Volume | 例如 `/var/lib/zhiwei` |

⚠️ 不挂卷的话：每次重新部署都会生成**新的 CA**，所有节点全部掉线。

---

## 4. 托管平台的部署形态（**已实装**）

主流 PaaS 的 Web Service 都在**边缘终止 TLS**，且不会把客户端证书转发进容器：

```
node ──HTTPS──> PaaS 边缘（解开 TLS）──明文 HTTP──> 容器
```

所以节点身份**不能靠 mTLS 客户端证书**，否则到平台就失效。知微的做法是
「**签名而非凭据**」：

1. 节点 enroll 时提交自己的 Ed25519 签名公钥（不再提交 CSR），monitor 存库
2. 此后每个请求（telemetry / inventory / 拉命令 / 交回执）都带一组签名头：
   `x-zhiwei-node` / `x-zhiwei-timestamp` / `x-zhiwei-nonce` / `x-zhiwei-signature`，
   签名覆盖「方法 + 路径(含 query) + 时间戳 + nonce + 请求体」
3. monitor 校验签名 + 时间窗（±300 秒）+ nonce 未重放（见 `crates/common/src/auth.rs`）
4. 命令通道双向签名：ops 签命令、节点验签；节点签回执、monitor 验签

于是：

| 场景 | 部署方式 | 节点连接地址 |
| --- | --- | --- |
| 自建主机 / 内网 | 默认（monitor 自己终结 TLS，不要求客户端证书） | `https://monitor.example.com` |
| Render / Railway / Northflank | 加 `--plain-http`（或 `ZHIWEI_PLAIN_HTTP=1`），TLS 由边缘终结 | 平台签发的 `https://<app>.onrender.com` |

明文 HTTP 不影响鉴权强度：身份来自签名，不来自传输层；边缘仍提供 HTTPS，
节点到边缘这一段依旧是加密的。

#### 自动检测（v0.1.0+）

不传 `--plain-http` 也不设 `ZHIWEI_PLAIN_HTTP` 时，monitor 会按以下环境变量
自动判断并默认开启明文 HTTP：

| 平台 | 触发变量 | 是否还会注入 `PORT` |
| --- | --- | --- |
| Render | `RENDER=true` | 会 |
| Railway | 任意 `RAILWAY_*`（`RAILWAY_ENVIRONMENT_NAME` 等） | 会 |
| Heroku | `DYNO` | 会 |
| Northflank | **没有可靠标记** | **不会** |

显式传 `ZHIWEI_PLAIN_HTTP=0` 总是覆盖自动判断（自建主机想保留 TLS 本地终结时用）。

**自动检测只是便利，不是保证。** 平台换一套内部约定它就会失效，所以托管平台
一律建议**显式配**这两个变量，别赌自动判断：

| 环境变量 | 值 | 作用 |
| --- | --- | --- |
| `ZHIWEI_PLAIN_HTTP` | `1` | 关掉本地 TLS，交给边缘终结 |
| `ZHIWEI_LISTEN` | `0.0.0.0:<容器端口>` | 绑所有网卡；只绑回环的话边缘够不着 |

`ZHIWEI_LISTEN` 也可以换成设 `PORT=<容器端口>`——检测到 `PORT` 时会自动绑
`0.0.0.0:<PORT>`。**Render / Railway / Heroku 会替你注入 `PORT`，Northflank 不会**，
所以在 Northflank 上这两个变量都得自己加。

#### Northflank 实战（踩过的坑）

Northflank 既不注入 `PORT`、也没有可用的环境变量前缀，于是两件事都走了自建默认值：

1. 监听落到 `127.0.0.1:8443`，边缘从容器外连不进来 → 健康检查失败 / 502
2. `plain_http` 判为 false，容器按 TLS 处理握手，而边缘转发来的是明文
   → `TLS handshake failed ... InvalidContentType`

它的日志长这样。注意 `paas_auto_detected=true` 只表示「你没显式配」，**不代表判对了**：

```
INFO zhiwei_monitor: starting zhiwei-monitor listen=127.0.0.1:8443
     plain_http=false paas_auto_detected=true
```

**做法**：Service → Environment 加两条，然后重新部署：

```
ZHIWEI_PLAIN_HTTP=1
ZHIWEI_LISTEN=0.0.0.0:8443
```

端口要和 Northflank 服务里配的 Port、以及镜像的 `EXPOSE 8443` 一致。改完日志里
应该是 `listen=0.0.0.0:8443 plain_http=true`，且不再有回环告警。

> v0.1.0+ 起，monitor 发现绑的是回环地址会主动打一条 WARN——就是为了让这个坑
> 在启动日志里一眼可见，而不是等健康检查失败了再去猜。

**改环境变量后必须重新部署。** 环境变量是注入到容器进程里的，改完不重启等于没改。
判断有没有生效，看启动那行：

```
starting zhiwei-monitor ... listen=0.0.0.0:8443 plain_http=true   ← 对了
starting zhiwei-monitor ... listen=127.0.0.1:8443 plain_http=false
                             paas_auto_detected=true              ← 变量没生效
```

`paas_auto_detected=true` 只说明「`ZHIWEI_PLAIN_HTTP` 不存在」，不代表判对了。
另外 `ZHIWEI_PLAIN_HTTP` 只接受 `1/0/true/false/yes/no/on/off`，别填 `auto`。

配错时的另一条线索是健康检查每 60 秒戳一次 TLS 端口：

```
WARN TLS 握手失败：对方发的是明文 HTTP，而本进程按 TLS 处理。
     部署在托管平台（边缘已终结 TLS）后面时，请设 ZHIWEI_PLAIN_HTTP=1 并重新部署
```

v0.1.0+ 起这条会带上下一步动作，并且**同类告警限流为每分钟一条**（其余降到 DEBUG），
免得把启动信息刷掉。

#### Northflank 上「部署成功但没有域名」

Northflank 只在**端口被标为 Public** 时才分配域名，格式是
`[port-name]--[service-name]--[random].code.run`。而且**只有 HTTP / HTTP2
协议的端口才能公开**——TCP / UDP 要另外买 L4 负载均衡器，拿不到这种域名。

检查顺序：**Run → Networking** 看端口列表

1. 没有 8443 这一条 → 点 **Detect ports**（Northflank 会扫镜像的 `EXPOSE`）
2. 有 8443 但 Protocol 是 TCP → 改成 **HTTP**
3. Protocol 对了但还是没域名 → 把 **Accessibility 改成 Public**（默认是 Private）

端口配置改了**不用重启**服务，保存后域名立刻出现。详情见 Northflank 官方文档
[Configure ports](https://northflank.com/docs/v1/application/network/configure-ports)。

> 这里有个容易踩的坑：**`EXPOSE` 的写法决定默认可见性**。
> `EXPOSE 8443` 会被当成 HTTP 且默认 public；写成 `EXPOSE 8443/tcp` 就变成
> TCP + private，域名永远出不来。本仓库的 Dockerfile 用的是前者，别改。

> 没配自动检测时的症状：Render 把明文 HTTP 转发给容器，容器却按 TLS 处理握手，
> 日志里出现 `TLS handshake failed error=received corrupt message of type
> InvalidContentType`——边缘发的 `GET /healthz` 被容器当成 ClientHello 解析。
> 修法就是打开明文 HTTP（`ZHIWEI_PLAIN_HTTP=1`）。

### 命令通道在托管平台上的当前状态

托管平台的部署只起了一个 Web Service（`zhiwei-monitor`），**没有起 ops-server**。
后果是 monitor 启动时读不到 `data/ops.pub`（或读到的内容为空），enroll 时不下发
`ops_public_key`，节点 `state.ops_public_key` 一直是 `None` —— 节点日志里会出现：

```
WARN zhiwei_node::control: 未持有 ops 公钥，控制通道不会拉取命令（安全侧默认拒绝）
```

这是**设计上的安全默认**：拿不到 ops 公钥 = 没法验签命令 = 不可能执行任何
来自 monitor 的「杀进程 / 重启主机 / 停容器」之类的写操作。如果你暂时不需要
远程命令通道（只想要 telemetry + 探活 + 证书扫描），这条 WARN 可以安全忽略。

要打开命令通道，需要把 ops-server 也部署起来，且让 monitor 能读到它的 `ops.pub`。
两条路线：

1. **同 Service 多进程**：在 `Dockerfile` 里同时启动 monitor 和 ops-server，让
   ops-server 把 `ops.pub` 写到 `ZHIWEI_DATA_DIR` 共享卷（最简单）
2. **拆 Service**：起一个独立的 `zhiwei-ops` Service，让它的 `ops.pub` 通过
   共享卷 / 外部存储（KMS / Secrets Manager / S3）传给 monitor —— 这种部署形态
   暂未实装，需要先在 `crates/ops-server/Cargo.toml` 加 Dockerfile + 在部署定义里
   加第二个 service。

无论哪条路线，节点侧不需要改 —— 一旦 monitor 把 `ops_public_key` 填进
`EnrollResponse`，节点就会自动写到 `state_dir/ops.pub` 并开始拉命令。

### 节点侧的 CA pinning（踩过一次的坑）

节点对 monitor 的 TLS 校验有两条路：

| 部署 | 节点信任根 | 来源 |
| --- | --- | --- |
| 自建（monitor 自己终结 TLS） | **pin monitor 的本地 CA** | enroll 响应下发 `ca_cert_pem`，节点存成 `<state>/ca.crt.pem` |
| 托管平台（边缘终结 TLS） | **系统根** | enroll **不下发** CA（本地 CA 与边缘的正经证书无关） |

规则是「**本进程终结 TLS 才下发本地 CA**」（`routes::enroll_ca_pem`）。这条曾经
没做到过：monitor 在托管平台上照样把自己的本地 CA 下发给节点，节点把它当唯一
信任根，于是**enroll 成功、之后每个请求都 TLS 校验失败**——失败出现在 enroll
之后，很容易误判成网络问题。

两个补救开关（`zhiwei-node`）：

- `--monitor-ca <path>`：用指定的 CA 覆盖 enroll 下发的那个（自建但用的是别的
  证书时用）
- `--monitor-ca -`：**强制不 pin、走系统根**。给「曾经对着自建 monitor 入网过、
  机器上留着旧 CA，现在改指向托管平台」的节点用——省得删掉 `node.id` 重新入网

> 自建部署若想额外加一层防御，可以让前置反代做 mTLS——但那是可选项，
> 不是节点入网的必要条件。

### 历史备选（未采用）

**TCP 穿透**（容器自己终结 TLS、平台暴露裸 TCP）曾作为备选：Railway / Northflank
支持，但 **Render 不支持公开裸 TCP**，且节点连不上平台自动签发的 HTTPS 证书。
已放弃。

---

## 5. 节点部署

node-agent 需要读取宿主机的 CPU / 内存 / 磁盘 / 网络，**不适合容器化**，
建议以二进制 + systemd 部署在每台被监控主机上：

### 装二进制（一行命令）

打好 tag（如 `v0.1.0`）后，release 工作流会为六个平台产出预编译包
（linux x86_64/aarch64 × musl/gnu、macOS arm64/x86_64）。安装脚本自动识别
系统与架构、下载、校验 SHA256、装到 `/usr/local/bin`：

```sh
curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/install.sh | sh
```

Linux 默认装 musl 静态版（不挑 glibc 版本，老发行版也能跑）。常用开关：

```sh
# 装 monitor 而不是 node-agent
... | sh -s -- --bin monitor

# 指定版本 / 目录 / 用动态链接版
... | sh -s -- --version 0.1.0 --dir ~/.local/bin --libc gnu

# 走自建制品仓库（国内加速，每次发版由 release.yml 自动同步）
ZHIWEI_BASE_URL=https://artifacts.hancic.site/releases/hancic128/zhiwei ... | sh
```

> **仓库现在是私有的**：匿名 `curl` 拿不到 raw 文件和 release 资产。开源前想用这条
> 命令，要么把仓库设为 public，要么 `export GITHUB_TOKEN=<PAT>` 后重跑
> （脚本会把 token 带上请求）。

不想用预编译包的话，本地编也一样：

```sh
cargo build --release --bin zhiwei-node
install -m 0755 target/release/zhiwei-node /usr/local/bin/
```

### 入网

```sh
# 首次：入网
ZHIWEI_MONITOR_URL=https://monitor.example.com \
ZHIWEI_BOOTSTRAP_TOKEN=<入网令牌> \
zhiwei-node --state-dir /var/lib/zhiwei-node --interval 30

# 之后：重启免 enroll（状态已落盘）
zhiwei-node --state-dir /var/lib/zhiwei-node --interval 30
```

> 想让节点在 SSH 退出、服务器重启后都持续上报？用 `install-node-service.sh`
> 自动装 systemd unit / launchd plist，**只需要传一个 token**：
>
> ```sh
> curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/install-node-service.sh \
>   | sudo sh -s -- --token zhi-bt-xxxxxxxx
> ```
>
> 卸载：`curl -fsSL ... | sudo sh -s -- uninstall`

> 同样也可以用独立脚本 `uninstall-node-service.sh`，效果一致但可单独下载：
>
> ```sh
> curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/uninstall-node-service.sh \
>   | sudo sh -s -- --purge --remove-binary
> ```
>
> `--purge` 连 state-dir 一起删（节点身份永久失效，要重新 enroll），`--remove-binary` 顺带 rm 二进制。

入网令牌怎么来见上面的
[「入网令牌从哪来」](#入网令牌从哪来zhiwei_bootstrap_token)——托管平台用
`ZHIWEI_BOOTSTRAP_TOKEN`，自建可以抢启动日志里那个 10 分钟的一次性 token。

节点本地状态：

| 文件 | 内容 | 权限 |
| --- | --- | --- |
| `signing.key` | Ed25519 签名私钥（32 B） | 0600 |
| `node.id` | monitor 分配的节点 ID | — |
| `ops.pub` | ops 控制平面公钥（enroll 时 TOFU 存下） | — |
| `ca.crt.pem` | monitor CA 证书（**仅自建部署**下会有这个文件；托管平台走系统根，见第 4 节） | — |

---

## 6. 上线前检查清单

- [ ] `<data-dir>` 已挂持久卷，且 `ZHIWEI_DATA_DIR` 指向它
- [ ] **CA 私钥已备份**（丢了就得重 enroll 全部节点）
- [ ] 镜像按 `--platform linux/amd64` 构建（Apple Silicon 上默认是 arm64）
- [ ] 托管平台部署已显式设 `ZHIWEI_PLAIN_HTTP=1`（别赌自动检测，Northflank 上它不生效）
- [ ] 托管平台已设 `ZHIWEI_LISTEN=0.0.0.0:<端口>`（或让平台注入 `PORT`），启动日志无回环告警
- [ ] Northflank：端口标为 **HTTP + Public**，确认 `*.code.run` 域名已分配
- [ ] 控制台凭据已就位：自建看首启日志的 `[ADMIN TOKEN]`，
      托管平台（尤其没有 Shell 的免费层）用 `ZHIWEI_ADMIN_TOKEN` 固定
- [ ] 已确认节点能连到 monitor 的地址（含防火墙 / 安全组）
- [ ] `RUST_LOG` 已按需调整，避免生产环境刷 debug 日志
- [ ] 固定入网令牌 ≥16 字符（建议 ≥32）；节点入网后删掉变量并重启更稳

---

## 8. 节点入网（托管平台，不用下载代码）

早期文档里出现过 `./scripts/dev.sh start` 这类命令——那是**本地开发**用的。
部署到 Render / Northflank 之后，用户手上没有仓库代码，也不该去翻构建日志。
正确的入网路径是**在控制台里生成一条命令**：

### 8.1 生成入网命令

1. 打开控制台 →「设置 → 入网令牌」→「新建入网令牌」
2. 选 TTL（1 小时 / 24 小时 / 7 天，默认 24 小时），可选填一个 label（例如 `prod-web-01`）
3. 点「创建」，弹窗会给出整段**可直接复制**的命令：

```sh
curl -sSL https://<your-monitor>/install-node.sh \
  | ZHIWEI_MONITOR_URL=https://<your-monitor> \
    ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxxxxxxx \
    bash -s
```

其中 `<your-monitor>` 由后端从请求的 `X-Forwarded-Proto` + `Host` 推断——
托管平台边缘会注入这两个头，所以生成出来的就是平台签发的 HTTPS 域名，
不需要在部署面板里额外配置。

> 命令里的 token 是**一次性凭据**：TTL 到期后自动失效。
> 也可以在「入网令牌」列表里手动撤销，撤销立即生效。

### 8.2 在目标机器上执行

要求 **root**（脚本要写 `/usr/local/bin` 和 systemd unit / launchd plist）：

```sh
curl -sSL https://<your-monitor>/install-node.sh | sudo -E bash -s
```

> 国内机器或出网受限环境，另见[第 9 节「国内 / 隔离网络部署」](#9-国内--隔离网络部署)——
> 需要额外带 `ZHIWEI_BASE_URL` 把下载换到自建源。

脚本做的事：

1. 从 GitHub Release 下载 `zhiwei-<target>.tar.gz`（内含 `zhiwei-node` 与 `VERSION`）；
   设了 `ZHIWEI_BASE_URL` 则改从自建源下载
2. 校验 `.sha256`（有就校验，没有就跳过）
3. 装二进制到 `/usr/local/bin/zhiwei-node`
4. 写 `/etc/zhiwei-node.env`（mode `0600`，含 `ZHIWEI_MONITOR_URL` + `ZHIWEI_BOOTSTRAP_TOKEN`）
5. 装常驻服务并设开机自启：
   - Linux：`/etc/systemd/system/zhiwei-node.service`，`systemctl enable --now`
   - macOS：`/Library/LaunchDaemons/com.zhiwei.node.plist`，`launchctl bootstrap system`
     （用 LaunchDaemon 而不是 LaunchAgent：脚本本来就要求 root，daemon 不依赖
     「有人登录桌面」也开机自启。plist 是 0644，**不内嵌令牌**——它先 `source`
     那个 0600 的 env 文件，再 exec 二进制，等价于 systemd 的 `EnvironmentFile`）
6. 节点随后出现在控制台「节点」页

支持 Linux（`x86_64` / `aarch64`，systemd）与 macOS（`arm64` / `x86_64`，launchd）。

卸载：

```sh
curl -sSL https://<your-monitor>/install-node.sh | sudo bash -s -- --uninstall
```

保留 `/var/lib/zhiwei-node`（节点的签名私钥与 `node.id`）——
重装不会换身份，也就不会在控制台里多出一个「幽灵节点」。
要连数据一起清理，手动 `rm -rf /var/lib/zhiwei-node`。

### 8.3 长期入网令牌（可选）

如果不想每次生成一次性命令（例如内网里批量铺节点），可以设环境变量
`ZHIWEI_BOOTSTRAP_TOKEN=<一个 >=16 字符的随机串>`：

- 它在**整个进程生命周期内长期有效**（重启也不变，因为来自环境变量）
- 在「入网令牌」列表里会标成 `长期`
- **撤销方式**：删掉环境变量并重新部署

> 一次性令牌和长期令牌共用同一个 `BootstrapTokens` 存储，
> 但一次性令牌只活在内存里，重启即失效——这也是为什么清单里建议
> 托管平台用一次性令牌：泄漏窗口更小。

### 8.4 排查

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 命令执行后 404 | release 还没产出该平台资产 | 确认 GitHub Release 里有 `zhiwei-<target>.tar.gz` |
| `install-node.sh` 下载到 HTML | 反代把脚本路径也喂给了 SPA 兜底 | 确认 `/install-node.sh` 是 monitor 在服务（不是 CDN 覆盖） |
| 节点没出现在控制台 | token 过期 / 已被撤销 / 网络不通 | 看 `/var/log/syslog` 里的 `zhiwei-node` 日志 |
| 卸载后重装多出一个节点 | 手动删过 `/var/lib/zhiwei-node` | 那等于换身份，属预期；不删数据目录则不会 |

---

## 9. 国内 / 隔离网络部署

默认形态有两个隐含依赖，国内机器（或任何出网受限的机器）都不满足：

| 依赖 | 默认值 | 国内实际体验 |
| --- | --- | --- |
| monitor 地址 | 托管平台域名（如 `zhiwei.onrender.com`） | 连不上 |
| 二进制下载 | `github.com/.../releases/latest/download/...` | 数十 KB/s 到超时 |

两处都可以换掉，**不需要改代码**，靠环境变量：

1. **monitor 自建在国内主机**（Render / Northflank 那套是给公网用户用的）
2. **下载源换成自建制品仓库**（`ZHIWEI_BASE_URL`）

### 9.1 步骤一：在国内主机起 monitor

任意一台国内 Linux 都能跑（SQLite，无外部依赖）。注意公网部署要给它一个域名，
否则节点侧拿 HTTPS 会失败：

```sh
# 在国内主机 A 上
curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/install.sh \
  | ZHIWEI_BIN=monitor \
    ZHIWEI_BASE_URL=https://artifacts.hancic.site/releases/hancic128/zhiwei \
    sh -s -- --version 0.1.0-alpha.2
```

> `install.sh` 在目标目录不可写时会自己 `sudo`，不需要手工加。
> `ZHIWEI_BASE_URL` 同样适用（国内直连，不然 GitHub 拉二进制会很慢）。

详细参数（数据目录、监听地址、admin token）见第 2 节。跑起来后控制台入口是
`http://<A>:8081`（或 `ZHIWEI_LISTEN` 指定的地址）。

> **国内机器起 monitor 的两个前提**：① 有公网 IP 或内网可达；② 走 HTTPS 需自备证书，
> 或者用前置 nginx 终结 TLS（`ZHIWEI_PLAIN_HTTP=1` + 反代）。
> 节点侧对 monitor 的地址只做 `https://` 前缀校验，不做 CA pinning（见第 4 节末），
> 所以换成任何可信域名都行。

### 9.2 步骤二：把自建下载源配在 monitor 上（推荐）

`install-node.sh` 支持 `ZHIWEI_BASE_URL`（与 `install.sh` 同一套语义），把节点
二进制的下载从 GitHub 换到自建仓库。

**在 monitor 上设一次 `ZHIWEI_NODE_BASE_URL`**，之后控制台生成的每条入网命令
都会自动带上这一行——执行者不用记得加：

```sh
# monitor 侧（9.1 那台）
ZHIWEI_NODE_BASE_URL=https://artifacts.hancic.site/releases/hancic128/zhiwei
```

配好后控制台生成的命令长这样（多了一行，其余不变）：

```sh
curl -sSL https://zhiwei.<国内域名>/install-node.sh \
  | ZHIWEI_MONITOR_URL=https://zhiwei.<国内域名> \
    ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxxxxxxx \
    ZHIWEI_BASE_URL=https://artifacts.hancic.site/releases/hancic128/zhiwei \
    sudo -E bash -s
```

不用这个变量的场合（临时 / 单机）也可以手工加：

```sh
curl -sSL https://zhiwei.<国内域名>/install-node.sh \
  | ZHIWEI_BASE_URL=https://artifacts.hancic.site/releases/hancic128/zhiwei \
    ZHIWEI_MONITOR_URL=https://zhiwei.<国内域名> \
    ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxxxxxxx \
    sudo -E bash -s
```

| 变量 | 作用 |
| --- | --- |
| `ZHIWEI_MONITOR_URL` | 入网目标，控制台生成时已经填好（国内 monitor 的域名） |
| `ZHIWEI_BOOTSTRAP_TOKEN` | 一次性入网凭据，控制台生成时已经填好 |
| `ZHIWEI_BASE_URL` | 二进制下载源。monitor 设了 `ZHIWEI_NODE_BASE_URL` 就自动带上；否则手工加 |
| `ZHIWEI_VERSION` | 可选，指定版本（如 `0.1.0-alpha.2`）；不设则取 `latest/` |

自建源的目录形状必须与 GitHub Release 一致：

```
<ZHIWEI_BASE_URL>/latest/<asset>            # 或
<ZHIWEI_BASE_URL>/v<version>/<asset>
```

`hancic-artifacts` 由 zhiwei 的 release 工作流在每次打 tag 时自动同步，形状天然对齐。

> **改了 `install-node.sh` 必须重新构建 monitor**：脚本是编译期内嵌进二进制的
> （`include_str!`），`GET /install-node.sh` 吐的永远是构建时那一份。
> 单一来源在仓库根的 `scripts/install-node.sh`，不存在副本漂移。

### 9.3 步骤三（可选）：长期令牌

内网批量铺节点时，不想每次去控制台点生成，就设长期令牌：

```sh
# monitor 侧
ZHIWEI_BOOTSTRAP_TOKEN=<>=32 字符随机串>

# 节点侧
curl -sSL https://zhiwei.<国内域名>/install-node.sh \
  | ZHIWEI_BASE_URL=... \
    ZHIWEI_MONITOR_URL=https://zhiwei.<国内域名> \
    ZHIWEI_BOOTSTRAP_TOKEN=<同一个串> \
    sudo -E bash -s
```

### 9.4 国内部署排查

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| `curl: (7) Failed to connect` 到 monitor | monitor 只在境外 | 按 9.1 在国内起一个 |
| 下载卡住 / 数十 KB/s | 走了 GitHub Releases | 带上 `ZHIWEI_BASE_URL`（9.2） |
| `下载失败: .../v1.2.3/...` | 自建源没同步该 tag | 确认 release 工作流的 artifacts job 成功，或去掉 `ZHIWEI_VERSION` 用 `latest` |
| 节点能连 monitor 但控制台看不到 | 国内 monitor 的 `ZHIWEI_LISTEN` 绑了回环 | 见第 2 节，托管/公网场景要 `0.0.0.0:<port>` |
| 入网命令里的域名是境外域名 | 生成命令时访问的是境外 monitor | 在国内 monitor 的控制台里重新生成 |
