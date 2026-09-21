# node-agent HTTP 客户端 chunked 解析修复

- **状态**: 进行中
- **日期**: 2026-09-21
- **触发**: 节点 `VM-16-12-opencloudos` 日志
  ```
  WARN zhiwei_node::probes: 拉取探针配置失败 error=解析探针配置失败
  DEBUG zhiwei_node: 拉取证书配置失败，本轮只用本机基线路径
        error=trailing characters at line 1 column 2
  WARN zhiwei_node::control: 未持有 ops 公钥，控制通道不会拉取命令
  ```

## 根因

`crates/node-agent/src/http.rs::send()` 用裸 `read_to_end` + 按
`\r\n\r\n` 切 header,不识别 HTTP/1.1 chunked transfer-encoding。

复现:

```
$ printf 'GET /healthz HTTP/1.1\r\nHost: zhiwei.onrender.com\r\nConnection: close\r\n\r\n' \
    | openssl s_client -servername zhiwei.onrender.com -connect zhiwei.onrender.com:443 -quiet -ign_eof
HTTP/1.1 200 OK
Transfer-Encoding: chunked       ← Render 边缘在 HTTP/1.1 + close 时强制 chunked
Connection: close
...

2
ok
0

```

`body = b"2\r\nok\r\n0\r\n\r\n"`,喂给 `serde_json` → `2` 解析为数字,
`\r` 在第 2 列 trailing → `trailing characters at line 1 column 2`,
正好对上节点日志。

batch / inventory 没翻车是 204 No Content(空 body)或 protobuf 不解析 JSON。

## 范围

- 必修:`crates/node-agent/src/http.rs::send()` 替换为按 `Content-Length` /
  `Transfer-Encoding: chunked` / close-delimited 三种 framing 解析
- 加单测:`http.rs` 模块内 `#[cfg(test)]`,覆盖 chunked / content-length /
  close-delimited / 多 chunk / chunk extension / trailer
- smoke:`scripts/smoke-chunked.sh` 用本地 HTTP 服务模拟 Render 的 chunked
  行为,辅助手动回归
- 文档:`docs/DEPLOY.md` 加一段:Render 单 Service 部署 = 控制通道不会工作

## 关键决策

- **不引入 hyper-util client**: 当前 `HttpTransport` 是自实现最小栈,
  chunked decoder 也自己写(~50 行),依赖只有 `tokio::io::BufReader` +
  `read_until`,避免拖入新依赖
- **BufReader 包一层 `AsyncRead`**: `read_until` / `read_exact` /
  `read_to_end` 都能复用,不用纠结 Buffer 残留字节
- **chunked 严格按 RFC 7230**: 支持 chunk extension (`5;ext=val`)、trailers

## 下一步

1. 在 `http.rs` 写单测(应该先红)
2. 改 `send()` → `read_response()` + `decode_chunked_body()`
3. `cargo test -p zhiwei-node-agent` 全绿
4. 跑 `scripts/smoke-chunked.sh` 手动验证
5. 同步给 `docs/DEPLOY.md`
