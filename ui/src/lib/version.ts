/**
 * Console version number.
 *
 * Intentionally hardcoded, doesn't read `/v1`'s `version` field: that's a request requiring auth first,
 * so on first frame / offline / token expiry the badge flickers or doesn't show at all. The version number
 * is a "build artifact constant", unrelated to runtime state — update here on release (in sync with git tag,
 * Cargo.toml's workspace version, ui/package.json).
 */
export const APP_VERSION = "v0.0.1";
