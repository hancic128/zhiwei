/** @type {import('tailwindcss').Config} */
// 令牌定义严格遵循 Trilium「前端约束规范」02-设计令牌 / 03-主题系统。
// brand 走 CSS 变量（随 data-theme 切换），surface / ink 固定不随主题变。
export default {
  darkMode: "class",
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        brand: {
          50: "var(--brand-50)",
          100: "var(--brand-100)",
          500: "var(--brand-500)",
          600: "var(--brand-600)",
          700: "var(--brand-700)",
          900: "var(--brand-900)",
        },
        surface: {
          0: "#ffffff",
          1: "#fafafa",
          2: "#f4f4f5",
          3: "#e4e4e7",
          4: "#d4d4d8",
        },
        ink: {
          900: "#18181b",
          700: "#3f3f46",
          500: "#71717a",
          400: "#a1a1aa",
        },
      },
      fontFamily: {
        sans: [
          "Inter",
          "-apple-system",
          "BlinkMacSystemFont",
          "Segoe UI",
          "PingFang SC",
          "Hiragino Sans GB",
          "Microsoft YaHei",
          "sans-serif",
        ],
        mono: [
          "ui-monospace",
          "SFMono-Regular",
          "Menlo",
          "Consolas",
          "monospace",
        ],
      },
      keyframes: {
        "panel-slide": {
          from: { opacity: "0", transform: "translateY(4px)" },
          to: { opacity: "1", transform: "translateY(0)" },
        },
      },
      animation: {
        "panel-slide": "panel-slide 200ms ease-out both",
      },
    },
  },
  plugins: [],
};
