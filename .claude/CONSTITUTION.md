# 项目宪法

## 编码规范

- 使用 Rust 进行后端开发
- 使用 React + TypeScript 进行前端开发
- 遵循 Rust 官方代码格式 (rustfmt)
- 前端遵循 ESLint + Prettier 规范

## Git 规范

- 使用 Conventional Commits 格式
- 提交前运行 lint + test
- 使用 husky 进行 pre-commit 检查

## 项目结构

- `crates/` - Rust 后端代码
- `ui/` - React 前端代码
- 领域实体 (Probe, Service, Channel 等) 作为顶层概念
