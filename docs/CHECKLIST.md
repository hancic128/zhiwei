# 开源准备清单

> 把知微（ZhiWei）从「私域项目」推到「公开仓库」需要补齐的所有 artifacts。
> 按优先级分组，每项标注：**必做 / 推荐 / 可选**。

## 0. 定位与边界（必做 · 最上游）

artifacts 齐了只说明「看起来很规范」，说明不了「别人为什么要 star」。
先定 **[POSITIONING.md](./POSITIONING.md)**（为谁做 / 凭什么 / 不做什么），
再往下补 artifacts；与定位冲突的功能请求默认不做。

- [x] `docs/POSITIONING.md`：一句话定位、第一用户、三个支柱、明确不做、影响力来源
- [x] README 首段与 POSITIONING 的一句话对齐（中英各一句）
- [x] 把「明确不做」清单搬进 `CONTRIBUTING.md`，作为 issue 拒收的公开依据

## 1. 法律与合规（必做）

- [ ] **LICENSE**：Apache-2.0（推荐，含专利授权；适合基础设施类项目；与 Rust 生态常见 license 兼容）
- [ ] **NOTICE**：第三方依赖致谢（cargo-deny 自动生成后人工审一遍）
- [ ] **DCO**（Developer Certificate of Origin）：每个 commit 加 `Signed-off-by:`（用 DCO bot 自动检查）
- [ ] 确认所有代码原创 / 已授权（避免公司内部代码带出）
- [ ] 确认依赖均为合法 license（cargo-deny 配置 + CI）

## 2. 项目门面（必做）

- [x] **对外身份统一 `hancic128`**：仓库 owner、git author name、README 署名、社交预览一律如此，不再对外出现 `Angryshark708` / `shark708` 等旧名（全局规则，2026-09-19 确立；全局 git `user.name` 已改）
- [ ] **README.md**（中英双语）：项目名、知微出处、一句话介绍、5 秒截图、Quick Start、特性、架构图、贡献链接、License 徽标
- [ ] **README.zh-CN.md**（中文独立版，或与上者合并）
- [ ] **LOGO + Banner**（GitHub social preview）：512×512 logo、1280×640 banner
- [ ] **docs/index.md**（docs 站首页）：技术文档、架构、部署、运维、API

## 3. 社区治理（必做）

- [ ] **CODE_OF_CONDUCT.md**（Contributor Covenant v2.1，CC BY 4.0）
- [x] **CONTRIBUTING.md**：定位与边界（含「明确不做」）+ 开发流程、commit 规范、PR 流程（已完成；issue 模板说明待 `.github/` 建好后再补链接）
- [ ] **SECURITY.md**：漏洞报告邮箱 / 流程 / 响应 SLA / 修复 timeline
- [ ] **GOVERNANCE.md**：维护者列表、决策机制、晋升路径
- [ ] **MAINTAINERS.md**：committer / reviewer 名单与责任
- [ ] **CODEOWNERS**：路径 → reviewer 自动指派

## 4. Issue / PR 模板（必做）

- [ ] `.github/ISSUE_TEMPLATE/bug_report.yml`
- [ ] `.github/ISSUE_TEMPLATE/feature_request.yml`
- [ ] `.github/ISSUE_TEMPLATE/question.yml`
- [ ] `.github/PULL_REQUEST_TEMPLATE.md`
- [ ] `.github/ISSUE_TEMPLATE/config.yml`（禁用项 + 引导）

## 5. CI/CD（必做）

- [ ] `.github/workflows/ci.yml`：
  - cargo fmt --check
  - cargo clippy -- -D warnings
  - cargo test
  - cargo deny check（license / advisory）
  - UI: npm ci && npm run build && npm run lint
- [ ] `.github/workflows/release.yml`：
  - 多平台二进制构建（linux/amd64, linux/arm64, darwin/amd64, darwin/arm64）
  - Docker 镜像推送（ghcr.io/zhiwei/zhiwei-server 等）
  - Helm chart 打包
- [ ] `.github/workflows/docs.yml`：构建 mdbook / mintlify 站
- [ ] `.github/dependabot.yml`：cargo + npm + GitHub Actions 自动 PR
- [ ] `.github/labeler.yml`：PR 自动分类

## 6. 代码质量（推荐）

- [ ] `rustfmt.toml`：与 rust-lang/rust 风格保持一致
- [ ] `clippy.toml`：lint 阈值
- [ ] `deny.toml`：license 白名单 / advisory 阈值
- [ ] `.editorconfig`：跨编辑器一致
- [ ] MSRV（Minimum Supported Rust Version）写入 workspace Cargo.toml
- [ ] Pre-commit hooks（可选）
- [ ] 单元测试覆盖率门槛（codecov.io）

## 7. 文档（推荐）

- [ ] `docs/architecture.md`：架构图（C4 model / mermaid）
- [ ] `docs/deployment.md`：自托管 / Docker / Kubernetes / systemd
- [ ] `docs/operations.md`：日常运维 Runbook
- [ ] `docs/security.md`：威胁模型 + 加固建议（基于 SECURITY.md）
- [ ] `docs/api.md`：HTTP API + MCP 工具参考（auto-gen from proto）
- [ ] `docs/comparisons.md`：与 Prometheus / Grafana / Zabbix / Netdata 等对比
- [ ] `docs/roadmap.md`：未来 6-12 个月方向
- [ ] `docs/faq.md`：常见问题
- [ ] `docs/translations.md`：翻译指南与术语表
- [ ] 示例 demo（docker-compose 启动一个全栈 demo）

## 8. 发布与镜像（推荐）

- [ ] Docker images 发布到 ghcr.io：zhiwei-server、zhiwei-ops、zhiwei-node、zhiwei-ui
- [ ] Helm chart 发布到 GitHub Pages / OCI registry
- [ ] GitHub Releases 页面带校验和（sha256sum.txt + signature）
- [ ] Homebrew tap（可选，macOS）
- [ ] apt 源（可选，Debian/Ubuntu）

## 9. 官网与品牌（可选）

- [ ] docs 站（zhiwei.dev）：用 mdbook / VitePress / Astro
- [ ] 项目官网首页
- [ ] 社交账号：Twitter/X、Mastodon、微信公众号（可选）
- [ ] 中文社区：知乎专栏、CSDN（可选）
- [ ] Discord / Matrix 频道（可选）
- [ ] Roadmap 看板（GitHub Projects）

## 10. 项目元数据（推荐）

- [ ] `repository` 字段写入所有 crate 的 Cargo.toml
- [ ] description / keywords / categories 写入 crates.io metadata
- [ ] crates.io 发布：先 dry-run → staging → 正式
- [ ] GitHub Topics：`monitoring`, `observability`, `rust`, `kubernetes`, `docker`, `self-hosted`, `mcp`

---

## 实施顺序建议

| 阶段 | 内容 | 估时 |
| --- | --- | --- |
| Day 1 | LICENSE + NOTICE + README + LOGO + 代码质量配置 | 0.5d |
| Day 2 | 社区治理 5 份文档 + Issue/PR 模板 + CI | 1d |
| Day 3 | docs 站骨架 + 关键文档 5 篇 | 1d |
| Day 4 | 发布流程 + Docker 镜像 + 第一个 v0.1.0 tag | 0.5d |
| Day 5 | 软启动：私域公告、收集反馈、修小问题 | 1d |
| Day 6 | 公开推 GitHub trending / Hacker News / 中文社区 | 1d |

**总计：~1 周可推到公开**

---

## 风险与避坑

| 风险 | 缓解 |
| --- | --- |
| 名字已被占用 | 提前查 GitHub / crates.io / PyPI / npm |
| 依赖里有 GPL/AGPL | cargo-deny 卡住；如必需，单独隔离或换依赖 |
| 内部配置 / 密钥残留 | grep 全仓库 .env / secret / token / password；CI 加 secret-scanning |
| 公司代码带出 | 由原作者 review 首版；CI 检查作者邮箱非公司域 |
| CI 暴露内网 | GitHub-hosted runner；不引用内网镜像 |
| 早期 PR 太多处理不过来 | 模板引导 + bot 自动回复 + 招募 co-maintainer |
| 文档跟代码脱节 | docs 在 CI 中由 cargo doc / typedoc 自动生成 |

---

## 一句话总结

**Apache-2.0 + 完整治理文档 + 健全 CI + Docker 镜像可拉 + docs 站可读** = 开源准备就绪。
其余是「加分项」，可逐步补。
