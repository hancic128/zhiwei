#!/usr/bin/env python3
"""upload.py — upload release artifacts to Tencent COS.

被 .github/workflows/oss-release.yml 调用。

环境变量（由 caller 通过 workflow env: 块注入）：
  COS_BUCKET           必填，bucket 名
  COS_REGION           必填，如 ap-nanjing
  COS_SECRET_ID        必填
  COS_SECRET_KEY       必填
  COS_KEY_PREFIX       必填，不含 tag，如 hancic128/zhiwei
  COS_FILE_PATTERN     必填，空格分隔的 glob，如 'zhiwei-*.tar.gz zhiwei-*.tar.gz.sha256'
  COS_EXTRA_PATHS      可选，空格分隔的字面路径，目录递归
  COS_VERSION_TAG      必填，如 v0.1.0-alpha.2
  COS_INCLUDE_LATEST   可选，'true'/'false'，默认 true
  COS_MAKE_PUBLIC      可选，'true'/'false'，默认 true
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
        for p in glob.glob(pat):
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
    """计算一个本地文件对应的 COS key 列表。

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
    """构造 cos S3 客户端，凭证来自 env。"""
    secret_id = os.environ.get("COS_SECRET_ID")
    secret_key = os.environ.get("COS_SECRET_KEY")
    region = os.environ.get("COS_REGION")
    missing = [k for k, v in {
        "COS_SECRET_ID": secret_id,
        "COS_SECRET_KEY": secret_key,
        "COS_REGION": region,
    }.items() if not v]
    if missing:
        raise EnvironmentError(f"required env not set: {', '.join(missing)}")

    try:
        from qcloud_cos import CosConfig, CosS3Client
    except ImportError as e:
        raise ImportError("cos-python-sdk-v5 not installed; run `pip install cos-python-sdk-v5`") from e

    config = CosConfig(Region=region, SecretId=secret_id, SecretKey=secret_key)
    return CosS3Client(config)


def upload_all():
    """入口：读 env → 收集文件 → 计算 key → 上传 → 输出 summary。"""
    bucket = os.environ["COS_BUCKET"]
    key_prefix = os.environ["COS_KEY_PREFIX"]
    file_pattern = os.environ["COS_FILE_PATTERN"]
    extra_paths = os.environ.get("COS_EXTRA_PATHS", "")
    version_tag = os.environ["COS_VERSION_TAG"]
    include_latest = os.environ.get("COS_INCLUDE_LATEST", "true").lower() == "true"
    make_public = os.environ.get("COS_MAKE_PUBLIC", "true").lower() == "true"

    files = collect_files(file_pattern=file_pattern, extra_paths=extra_paths)
    if not files:
        print("::error::no files matched", file=sys.stderr)
        sys.exit(1)

    client = build_client()
    acl = "public-read" if make_public else ""

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
                    with open(path, "rb") as f:
                        kwargs = {"Bucket": bucket, "Key": key, "Body": f}
                        if acl:
                            kwargs["ACL"] = acl
                        client.put_object(**kwargs)
                    summary.append({"file": path, "key": key, "size": os.path.getsize(path)})
                    break
                except Exception as e:
                    if attempt == 2:
                        failures.append((path, key, str(e)))
                    else:
                        print(f"::warning::retry {attempt + 1}/3 {key}: {e}", file=sys.stderr)

    # 输出 step summary
    print("## OSS upload summary")
    for item in summary:
        print(f"- `{item['file']}` ({item['size']} B) → `cos://{bucket}/{item['key']}`")
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