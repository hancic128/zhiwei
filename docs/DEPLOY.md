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
不需要进容器。仓库自带的 [`render.yaml`](../render.yaml) 已经这么配了。

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
> 修法就是打开明文 HTTP。仓库根目录的 `render.yaml` 是开箱即用的 Blueprint。

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

# 走镜像站或内网分发
ZHIWEI_BASE_URL=https://mirror.example.com/zhiwei ... | sh
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
