// 语言包体检：i18next 的插值必须是 {{var}}，写成 {var} 会原样渲染。
// 这个坑已经踩过两次，用脚本挡住。
import { readFileSync, readdirSync } from "node:fs";

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
console.log(`OK 语言包检查通过（${dirs.join(", ")}，各 ${zhKeys.size} 个 key）`);
