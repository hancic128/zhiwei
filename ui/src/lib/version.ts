/**
 * 控制台版本号。
 *
 * 刻意写死、不走 `/v1` 的 `version` 字段：那是个需要先鉴权的动态请求，
 * 首帧 / 断网 / 令牌失效时徽章会闪一下或直接不显示。版本号属于「构建产物
 * 自带的常量」，跟运行状态没关系——发布时手动改这里（与 git tag、Cargo.toml 的
 * workspace version、ui/package.json 保持一致）。
 */
export const APP_VERSION = "v0.1.5";
