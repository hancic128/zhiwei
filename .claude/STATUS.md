# 项目状态

## [2026-10-07] MCP 对齐最新实现

### 现状
- MCP 工具集与 REST API 对齐完成（`crates/monitor-server/src/mcp.rs`）
- service layer 早已移除（spec 2026-10-04），MCP 里的 `list/create/update/delete_service`
  指向已不存在的 `/v1/services`，已删除，换成 `get_services_timeline`
- 修正的契约漂移：alert rule 由 `expr` 改为 `metric/op/threshold/duration_seconds`；
  channel 由 `type` 改为 `kind`(feishu/slack/bluebird/webhook) + `min_severity`；
  `test_channel` 改为按参数（`/v1/channels/test`）而非 channel_id；
  probe 改为 `kind`(http/tcp/tls) + `target_json/expect_json`；cert source 增加
  notify/enabled/all-nodes；`delete_node` 的 `force` 按布尔处理；数字 id 支持 int/string
- 工具总数 41 → 38；`cargo test --workspace` 全绿，clippy 干净

### 待办
- [ ] 如需，补充 MCP resources/prompts 能力（当前仅 tools/*）
