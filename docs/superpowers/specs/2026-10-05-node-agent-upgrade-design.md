# Node Agent 远程升级方案

**日期**: 2026-10-05
**状态**: 设计中

## 1. 背景与目标

node-agent 部署在多台主机上，需要支持从服务端远程升级，无需手动登录每台机器操作。

**目标**:
- 支持从 monitor-server 下载升级包
- 自动校验二进制完整性（SHA256）
- 支持自动回滚（升级失败时恢复到上一版本）
- 与现有命令执行框架集成（使用 ops-server 签名命令）

## 2. 架构概览

```
┌─────────────────────────────────────────────────────────────────────┐
│                         升级流程                                      │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Admin UI                    ops-server           monitor-server    │
│      │                            │                     │           │
│      │  1. 上传升级包              │                     │           │
│      │───────────────────────────►│                     │           │
│      │                            │  2. 写入文件系统      │           │
│      │                            │────────────────────►│           │
│      │                            │                     │           │
│      │  3. 点击升级                │                     │           │
│      │───────────────────────────►│                     │           │
│      │                            │  4. 创建签名命令      │           │
│      │                            │────────────────────►│           │
│      │                            │                     │           │
│      │                            │                     │ 5. 存储命令 │
│      │                            │                     │           │
│      └                            │                     │           │
│                                    │                     │           │
│                                    │                     │ ◄────────┤
│                                    │                     │ 6. GET   │
│                                    │                     │   /v1/   │
│                                    │                     │   commands│
│                                    │                     │           │
│                                    │                     │ 7. 下载   │
│                                    │                     │   binary │
│                                    │                     │─────────►│
│                                    │                     │           │
│                                    │                     │ 8. 校验   │
│                                    │                     │ 9. 备份   │
│                                    │                     │10. 替换   │
│                                    │                     │11. 重启   │
│                                    │                     │           │
│                                    │                     │12. 上报   │
│                                    │                     │   结果   │
│                                    │                     │◄─────────┤
│                                    │                     │           │
└─────────────────────────────────────────────────────────────────────┘
```

## 3. Proto 定义

### 3.1 新增 Action

```protobuf
// proto/control.proto

enum Action {
  // ... 现有 actions ...
  ACTION_UPGRADE_AGENT = 12;  // 升级 agent
}
```

### 3.2 新增 Params

```protobuf
// proto/control.proto

message UpgradeAgentParams {
  string version = 1;            // 目标版本（如 "v1.2.3"）
  string download_url = 2;       // 二进制下载 URL
  string sha256 = 3;             // 二进制 SHA256 校验和
  string backup_path = 4;        // 备份路径（可选，默认使用固定路径）
  bool restart = 5;              // 升级后是否自动重启（默认 true）
}
```

## 4. 升级包存储

### 4.1 目录结构

```
/opt/zhiwei/agent-upgrades/
├── index.json              # 版本索引
├── v1.0.0/
│   └── node-agent          # 二进制文件
├── v1.1.0/
│   └── node-agent
└── v1.2.3/
    └── node-agent
```

### 4.2 index.json 格式

```json
{
  "latest": "v1.2.3",
  "packages": {
    "v1.2.3": {
      "sha256": "abc123...",
      "size_bytes": 12345678,
      "uploaded_at": "2026-10-05T10:00:00Z"
    },
    "v1.2.2": {
      "sha256": "def456...",
      "size_bytes": 12300000,
      "uploaded_at": "2026-09-20T08:00:00Z"
    }
  }
}
```

## 5. 服务端实现

### 5.1 ops-server 新增 API

#### POST /v1/upgrade-packages
- **功能**: 上传新的升级包
- **认证**: 管理员 API key
- **请求体**: multipart/form-data，包含:
  - `binary`: 二进制文件
  - `version`: 版本号（如 "v1.2.3"）
- **响应**: 201 Created
- **逻辑**:
  1. 验证 version 格式
  2. 计算 SHA256
  3. 创建版本目录并写入 binary
  4. 更新 index.json
  5. 禁止重复版本号

#### GET /v1/upgrade-packages/latest
- **功能**: 获取最新版本信息
- **响应**:
```json
{
  "version": "v1.2.3",
  "download_url": "https://monitor:8443/v1/upgrade/v1.2.3",
  "sha256": "abc123..."
}
```

#### GET /v1/upgrade-packages
- **功能**: 获取所有可用版本列表
- **响应**:
```json
{
  "packages": [
    {"version": "v1.2.3", "sha256": "abc123...", "uploaded_at": "..."},
    {"version": "v1.2.2", "sha256": "def456...", "uploaded_at": "..."}
  ]
}
```

### 5.2 monitor-server 新增 API

#### GET /v1/upgrade/:version
- **功能**: 下载指定版本的二进制
- **认证**: 节点 Ed25519 签名
- **响应**: 二进制文件流（Content-Type: application/octet-stream）
- **错误**: 404 如果版本不存在

### 5.3 命令下发

升级命令通过现有的命令通道下发，ops-server 签名:

```json
{
  "id": "uuid",
  "node_id": "node-123",
  "action": "ACTION_UPGRADE_AGENT",
  "params_json": "{\"version\":\"v1.2.3\",\"download_url\":\"https://monitor:8443/v1/upgrade/v1.2.3\",\"sha256\":\"abc123...\",\"restart\":true}",
  "issued_at_unix_nano": 1234567890,
  "ttl_seconds": 300,
  "signature": "..."
}
```

## 6. Agent 端实现

### 6.1 execute_upgrade_agent 函数

```rust
async fn execute_upgrade_agent(cmd: &Command) -> anyhow::Result<Vec<u8>> {
    let p: UpgradeAgentArgs = serde_json::from_str(&cmd.params_json)?;

    info!(version = %p.version, "Starting agent upgrade");

    // 1. 下载新 binary 到临时位置
    let temp_path = format!("/tmp/node-agent-{}.bin", p.version);
    download_binary(&p.download_url, &temp_path).await?;

    // 2. 校验 SHA256
    let hash = sha256_file(&temp_path)?;
    if hash != p.sha256 {
        std::fs::remove_file(&temp_path)?;
        bail!("SHA256 mismatch: expected {}, got {}", p.sha256, hash);
    }

    // 3. 获取当前二进制路径
    let current_path = std::env::current_exe()?;
    let backup_dir = PathBuf::from("/var/lib/zhiwei-agent/backup");
    let backup_path = backup_dir.join(format!("node-agent.{}", p.version));

    // 4. 备份当前版本
    std::fs::create_dir_all(&backup_dir)?;
    std::fs::copy(&current_path, &backup_path)?;

    // 5. 原子替换
    // 使用 rename 确保原子性
    std::fs::rename(&temp_path, &current_path)?;

    // 6. 设置执行权限
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&current_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&current_path, perms)?;
    }

    // 7. 重启 agent（通过 systemd）
    if p.restart.unwrap_or(true) {
        tokio::process::Command::new("systemctl")
            .args(["restart", "node-agent"])
            .output()
            .await?;
    }

    Ok(format!("upgraded to {}, backup at {:?}", p.version, backup_path).into_bytes())
}
```

### 6.2 自动回滚机制

**方案**: 依赖 systemd 的 restart 机制

1. agent 替换二进制后，发送 `systemctl restart node-agent`
2. systemd 启动新版本
3. 如果新版本启动失败，systemd 根据配置的重试策略处理
4. 手动回滚命令（后续实现）可恢复到备份版本

**备份文件位置**: `/var/lib/zhiwei-agent/backup/node-agent.{version}`

## 7. 数据库扩展

### 7.1 新增表

```sql
-- 节点版本记录
CREATE TABLE node_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id TEXT NOT NULL,
    version TEXT NOT NULL,
    upgraded_at_unix_nano INTEGER NOT NULL,
    UNIQUE(node_id, version)
);

CREATE INDEX idx_node_versions_node_id ON node_versions(node_id);

-- 升级历史
CREATE TABLE upgrade_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id TEXT NOT NULL,
    from_version TEXT,
    to_version TEXT NOT NULL,
    status TEXT NOT NULL,  -- success, failed, rolling_back, rolled_back
    error TEXT,
    created_at_unix_nano INTEGER NOT NULL,
    finished_at_unix_nano INTEGER
);
```

## 8. UI 设计

### 8.1 节点详情页

```
┌─────────────────────────────────────────────────────────────┐
│  节点信息                                      [升级设置]    │
├─────────────────────────────────────────────────────────────┤
│  主机名: server-01                           状态: 在线     │
│  IP: 192.168.1.10                            版本: v1.2.2   │
│  标签: production                             最后活跃: 刚刚 │
├─────────────────────────────────────────────────────────────┤
│  [ 升级到 v1.2.3 ]                                          │
│                                                             │
│  可用版本: v1.2.3 (最新) | v1.2.2 (当前) | v1.2.1          │
├─────────────────────────────────────────────────────────────┤
│  升级历史                                                   │
│  ┌───────────────────────────────────────────────────────┐ │
│  │ 2026-10-05 10:00  v1.2.1 → v1.2.2  ✓ 成功            │ │
│  │ 2026-09-20 14:30  v1.2.0 → v1.2.1  ✓ 成功            │ │
│  │ 2026-09-01 09:00  v1.1.9 → v1.2.0  ✗ 失败 (SHA256)   │ │
│  └───────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

### 8.2 升级设置页

```
┌─────────────────────────────────────────────────────────────┐
│  Agent 升级设置                                              │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  升级包存储路径                                              │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ /opt/zhiwei/agent-upgrades                     [浏览]│   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
│  自动升级                                                   │
│  [✓] 允许自动升级到新版本                                    │
│                                                             │
│  升级策略                                                   │
│  ○ 手动升级（推荐）                                          │
│  ○ 补丁版本自动升级（1.2.x → 1.2.y）                          │
│  ○ 所有版本自动升级（不推荐）                                  │
│                                                             │
│  回滚设置                                                   │
│  [✓] 升级失败时自动回滚                                       │
│  最大备份版本数: [5]                                         │
│                                                             │
│                                    [保存设置]               │
└─────────────────────────────────────────────────────────────┘
```

## 9. 错误处理

| 错误类型 | 处理方式 |
|----------|----------|
| 下载失败 | 重试 3 次，间隔 5s，失败上报 |
| SHA256 不匹配 | 删除临时文件，上报校验失败 |
| 备份失败 | 中止升级，上报错误 |
| 替换失败 | 清理临时文件，上报错误 |
| systemd restart 失败 | 记录错误，提示手动处理 |
| 版本已存在 | ops-server 拒绝上传 |

## 10. 安全考虑

1. **命令签名**: 所有升级命令由 ops-server 私钥签名，agent 验证签名后才执行
2. **完整性校验**: SHA256 校验确保二进制未被篡改
3. **认证**: 节点通过 Ed25519 签名认证下载请求
4. **备份**: 升级前自动备份，支持回滚
5. **日志**: 完整的操作审计日志

## 11. 实现计划

### Phase 1: 核心功能
- [ ] Proto 定义扩展
- [ ] ops-server 升级包上传 API
- [ ] monitor-server 升级包下载 API
- [ ] agent 升级执行逻辑
- [ ] 数据库表扩展

### Phase 2: UI 集成
- [ ] 节点详情页升级按钮
- [ ] 版本信息显示
- [ ] 升级历史展示

### Phase 3: 增强功能
- [ ] 批量升级（选择多个节点）
- [ ] 升级进度追踪
- [ ] 回滚命令支持
