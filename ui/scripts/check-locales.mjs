// 语言包体检：i18next 的插值必须是 {{var}}，写成 {var} 会原样渲染。
// 这个坑已经踩过两次，用脚本挡住。
//
// 另外还要挡住「源码里 t("x.y") 但语言包里没有 x.y」——这种 key 中英文同时缺失，
// 中英一致性检查发现不了，页面上只会原样显示 "x.y"。用源码扫描兜住。
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const SINGLE_BRACE = /(?<!\{)\{([a-zA-Z_][a-zA-Z0-9_]*)\}(?!\})/g;
const dirs = readdirSync("locales", { withFileTypes: true })
  .filter((d) => d.isDirectory())
  .map((d) => d.name);

let failed = false;
for (const dir of dirs) {
  const path = `locales/${dir}/common.json`;
  const raw = readFileSync(path, "utf8");
  try {
    JSON.parse(raw);
  } catch (e) {
    console.error(`FAIL ${path}: JSON 语法错误 — ${e.message}`);
    failed = true;
    continue;
  }
  const hits = [...raw.matchAll(SINGLE_BRACE)];
  if (hits.length > 0) {
    console.error(
      `FAIL ${path}: 发现 ${hits.length} 处单花括号占位符（应为 {{var}}）：`,
    );
    for (const h of hits.slice(0, 10)) console.error(`   ${h[0]}`);
    failed = true;
  }
}

const [zh, en] = dirs.map((d) =>
  JSON.parse(readFileSync(`locales/${d}/common.json`, "utf8")),
);
const flat = (o, p = "") =>
  Object.entries(o).flatMap(([k, v]) =>
    typeof v === "object" && v !== null ? flat(v, `${p}${k}.`) : [`${p}${k}`],
  );
const zhKeys = new Set(flat(zh));
const enKeys = new Set(flat(en));
const missingInEn = [...zhKeys].filter((k) => !enKeys.has(k));
const missingInZh = [...enKeys].filter((k) => !zhKeys.has(k));
if (missingInEn.length || missingInZh.length) {
  console.error("FAIL 中英语言包 key 不一致：");
  if (missingInEn.length)
    console.error("   英文缺:", missingInEn.slice(0, 10).join(", "));
  if (missingInZh.length)
    console.error("   中文缺:", missingInZh.slice(0, 10).join(", "));
  failed = true;
}

if (failed) process.exit(1);

// ---------- 源码里的 t("key") 是否都在语言包里 ----------
// 只扫字面量 key；模板拼接的 t(`state.${x}`) 没法静态枚举，跳过。
// 复数形式（t("a.b", {count})）在语言包里存成 a.b_one / a.b_other，这里只校验前缀存在。

function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    const p = join(dir, entry);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (p.endsWith(".ts") || p.endsWith(".tsx")) out.push(p);
  }
  return out;
}

const used = new Map(); // key -> Set(files)
for (const file of walk("src")) {
  const raw = readFileSync(file, "utf8");
  for (const m of raw.matchAll(/\bt\(\s*["'`]([a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)+)["'`]/g)) {
    if (!used.has(m[1])) used.set(m[1], new Set());
    used.get(m[1]).add(file);
  }
}

const allKeys = new Set([...zhKeys, ...enKeys]);
const missing = [];
for (const [key, files] of used) {
  // t("a.b") 可能是复数：语言包里是 a.b_one / a.b_other
  const plural = [...allKeys].some(
    (k) => k.startsWith(`${key}_`),
  );
  if (!allKeys.has(key) && !plural) {
    missing.push(`${key}  (${[...files].join(", ")})`);
  }
}
if (missing.length) {
  console.error("FAIL 源码用到的翻译 key 在语言包里不存在：");
  for (const m of missing) console.error(`   ${m}`);
  process.exit(1);
}

console.log(
  `OK 语言包检查通过（${dirs.join(", ")}，各 ${zhKeys.size} 个 key；源码引用 ${used.size} 个 key 全部命中）`,
);

