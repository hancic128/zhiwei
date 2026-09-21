#!/usr/bin/env python3
"""upload.py — upload release artifacts to Aliyun OSS.

被 .github/workflows/oss-release.yml 调用（或 caller 仓库直接调用）。

环境变量（由 caller 通过 workflow env: 块注入）：
  OSS_ACCESS_KEY_ID      必填
  OSS_ACCESS_KEY_SECRET  必填
  OSS_ENDPOINT           必填，如 https://oss-cn-hangzhou.aliyuncs.com
  OSS_BUCKET             必填，bucket 名
  OSS_KEY_PREFIX         必填，不含 tag，如 hancic128/zhiwei
  OSS_FILE_PATTERN       必填，空格分隔的 glob，如 'zhiwei-*.tar.gz zhiwei-*.tar.gz.sha256'
  OSS_EXTRA_PATHS        可选，空格分隔的字面路径，目录递归
  OSS_VERSION_TAG        必填，如 v0.1.0-alpha.2
  OSS_INCLUDE_LATEST     可选，'true'/'false'，默认 true
  OSS_MAKE_PUBLIC        可选，'true'/'false'，默认 true
  OSS_NUM_THREADS        可选，分块上传并发数，默认 4
  OSS_PART_SIZE          可选，分块大小（字节），默认 1MB
"""
import fnmatch
import glob
import os
import sys
from typing import List


def collect_files(file_pattern: str, extra_paths: str) -> List[str]:
    """收集所有要上传的文件相对路径。

    Args:
        file_pattern: 空格分隔的 glob 列表
        extra_paths:  空格分隔的字面路径；目录自动递归

    Returns:
        相对路径列表（相对于 cwd，即 checkout 后的仓库根）

    Raises:
        ValueError: file_pattern 与 extra_paths 都为空
    """
    if not file_pattern and not extra_paths:
        raise ValueError("file_pattern is empty and extra_paths is empty; nothing to upload")

    matched: List[str] = []

    # 1. 二进制：glob + fnmatch
    for pat in file_pattern.split():
        for p in glob.glob(pat, recursive=True):
            if os.path.isfile(p) and fnmatch.fnmatch(p, pat):
                matched.append(p)

    # 2. 额外路径：字面路径或目录递归
    for path in extra_paths.split():
        if not path:
            continue
        if os.path.isdir(path):
            for root, _, files in os.walk(path):
                for f in files:
                    matched.append(os.path.join(root, f))
        elif os.path.isfile(path):
            matched.append(path)
        else:
            print(f"::warning::extras path not found, skipping: {path}", file=sys.stderr)

    # 去重 + 排序（确定性输出便于 review）
    return sorted(set(matched))


def compute_keys(
    key_prefix: str, version_tag: str, relative_path: str, include_latest: bool
) -> List[str]:
    """计算一个本地文件对应的 OSS key 列表。

    总是生成 `<prefix>/v<tag>/<path>`；当 include_latest=True 时再加 `<prefix>/latest/<path>`。
    key_prefix 的尾部 / 会被 strip；relative_path 的前导 ./ 也会 strip。
    """
    prefix = key_prefix.strip("/").strip()
    rp = relative_path.lstrip("./")
    # caller 传完整 tag（含 v 前缀），如 'v0.1.0-alpha.2'
    keys = [f"{prefix}/{version_tag}/{rp}"]
    if include_latest:
        keys.append(f"{prefix}/latest/{rp}")
    return keys


def build_client():
    """构造 OSS bucket 客户端，凭证来自 env。"""
    key_id = os.environ.get("OSS_ACCESS_KEY_ID")
    key_secret = os.environ.get("OSS_ACCESS_KEY_SECRET")
    endpoint = os.environ.get("OSS_ENDPOINT")
    bucket_name = os.environ.get("OSS_BUCKET")
    missing = [k for k, v in {
        "OSS_ACCESS_KEY_ID": key_id,
        "OSS_ACCESS_KEY_SECRET": key_secret,
        "OSS_ENDPOINT": endpoint,
        "OSS_BUCKET": bucket_name,
    }.items() if not v]
    if missing:
        raise EnvironmentError(f"required env not set: {', '.join(missing)}")

    try:
        import oss2
    except ImportError as e:
        raise ImportError("oss2 not installed; run `pip install oss2`") from e

    auth = oss2.Auth(key_id, key_secret)
    return oss2.Bucket(auth, endpoint, bucket_name)


def _upload_one(bucket, local_path: str, key: str, make_public: bool, threads: int, part_size: int):
    """上传单个文件到 OSS。用 resumable_upload（多线程分块）以提速。

    返回 ETag。
    """
    import oss2

    headers = {}
    if make_public:
        headers["x-oss-object-acl"] = "public-read"

    result = oss2.resumable_upload(
        bucket,
        key,
        local_path,
        headers=headers,
        num_threads=threads,
        part_size=part_size,
        store=oss2.ResumableStore(root="/tmp/.oss-upload-state"),
    )
    return result.etag


def upload_all():
    """入口：读 env → 收集文件 → 计算 key → 上传 → 输出 summary。"""
    key_prefix = os.environ["OSS_KEY_PREFIX"]
    file_pattern = os.environ["OSS_FILE_PATTERN"]
    extra_paths = os.environ.get("OSS_EXTRA_PATHS", "")
    version_tag = os.environ["OSS_VERSION_TAG"]
    include_latest = os.environ.get("OSS_INCLUDE_LATEST", "true").lower() == "true"
    make_public = os.environ.get("OSS_MAKE_PUBLIC", "true").lower() == "true"
    threads = int(os.environ.get("OSS_NUM_THREADS", "4"))
    part_size = int(os.environ.get("OSS_PART_SIZE", str(1024 * 1024)))

    files = collect_files(file_pattern=file_pattern, extra_paths=extra_paths)
    if not files:
        print("::error::no files matched", file=sys.stderr)
        sys.exit(1)

    bucket = build_client()

    summary: List[dict] = []
    failures: List[tuple[str, str, str]] = []

    for path in files:
        keys = compute_keys(
            key_prefix=key_prefix,
            version_tag=version_tag,
            relative_path=path,
            include_latest=include_latest,
        )
        for key in keys:
            for attempt in range(3):
                try:
                    etag = _upload_one(bucket, path, key, make_public, threads, part_size)
                    summary.append({"file": path, "key": key, "size": os.path.getsize(path)})
                    print(f"uploaded {path} -> oss://.../{key} ({etag})", file=sys.stderr)
                    break
                except Exception as e:
                    if attempt == 2:
                        failures.append((path, key, str(e)))
                    else:
                        print(f"::warning::retry {attempt + 1}/3 {key}: {e}", file=sys.stderr)

    # 输出 step summary
    print("## OSS upload summary")
    for item in summary:
        print(f"- `{item['file']}` ({item['size']} B) → `{item['key']}`")
    if failures:
        print(f"\n## Failures ({len(failures)})")
        for path, key, err in failures:
            print(f"- `{path}` → `{key}`: {err}")

    # 失败阈值：失败 ≥ 文件数 50% 即视为整体失败
    if failures and len(failures) >= len(files):
        sys.exit(1)

    if failures:
        print(f"::warning::{len(failures)} upload(s) failed but <50% threshold", file=sys.stderr)


if __name__ == "__main__":
    upload_all()
