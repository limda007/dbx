# Commit 规范文档

DBX 仓库的 Git 提交规范。基于 Conventional Commits，并在 type 前使用固定 emoji。

历史提交可能尚未统一带 emoji；**新提交必须遵守本文档**。

## 提交格式

### 标准格式

```
<emoji> <type>(<scope>): <subject>

[body]

[footer]
```

### 格式说明

- **emoji**: 对应提交类型的表情符号（必填）
- **type**: 提交类型（必填）
- **scope**: 修改内容的范围（可选）
- **subject**: 简要描述（必填）
- **body**: 详细描述（可选）
- **footer**: 需要关闭的 issue（可选）

### Schema 正则

```
(💎 bump|🚀 break|✨ feat|🐛 fix|♻️ refactor|⚡ perf|✅ test|🔨 build|👷 ci|📝 docs|🧹 chore)(\([a-z][a-z0-9_-]*\))?:(\s.*)
```

## 提交类型列表

### 1. ✨ feat - 新功能

- **描述**: 添加新功能
- **语义化版本**: MINOR（次版本号 +1）
- **示例**:
  ```
  ✨ feat: add connection lifecycle stage logging
  ✨ feat(export): include query SQL in XLSX
  ✨ feat(core): add DbOperationBudget facade
  ```

### 2. 🐛 fix - Bug 修复

- **描述**: 修复 Bug
- **语义化版本**: PATCH（修订号 +1）
- **示例**:
  ```
  🐛 fix: handle empty redis scan cursor
  🐛 fix(mongodb): preserve long ids in filters and updates
  🐛 fix(sync): refresh tunnel profiles after download
  ```

### 3. ♻️ refactor - 代码重构

- **描述**: 重构代码，不修复 bug 也不添加新功能
- **语义化版本**: PATCH（修订号 +1）
- **示例**:
  ```
  ♻️ refactor: simplify error handling
  ♻️ refactor(core): extract connection_lifecycle module
  ♻️ refactor(query): move budget types to lifecycle facade
  ```

### 4. ⚡ perf - 性能优化

- **描述**: 提升性能的代码更改
- **语义化版本**: PATCH（修订号 +1）
- **示例**:
  ```
  ⚡ perf: skip stage log allocation when debug is disabled
  ⚡ perf(postgres): reuse prepared statements for metadata
  ⚡ perf(export): stream large result sets
  ```

### 5. ✅ test - 测试相关

- **描述**: 添加或修正测试
- **语义化版本**: PATCH（修订号 +1）
- **示例**:
  ```
  ✅ test: add connection_lifecycle unit coverage
  ✅ test(integration): cover mysql text protocol select
  ✅ test(core): fix budget default timeout assertions
  ```

### 6. 🔨 build - 构建系统

- **描述**: 影响构建系统或外部依赖的更改
- **常用 scope**: cargo, pnpm, docker, nix, agents
- **语义化版本**: PATCH（修订号 +1）
- **示例**:
  ```
  🔨 build: update rust toolchain pin
  🔨 build(docker): optimize agent image layers
  🔨 build(pnpm): bump workspace lockfile
  ```

### 7. 👷 ci - CI 配置

- **描述**: CI 配置文件和脚本的更改
- **常用 scope**: github, pre-commit
- **示例**:
  ```
  👷 ci: add rust fmt check job
  👷 ci(github): parallelize agent asset uploads
  👷 ci(pre-commit): update hook versions
  ```

### 8. 📝 docs - 文档

- **描述**: 仅文档或注释变更（不改运行时行为）
- **语义化版本**: 无版本影响（或随发布说明）
- **示例**:
  ```
  📝 docs: add contributor build tutorial
  📝 docs(commit): adopt emoji conventional commits
  📝 docs(pips): add phase-a connection lifecycle plan
  ```

### 9. 🧹 chore - 杂项维护

- **描述**: 不直接影响功能/修复的维护性改动（版本 bump 脚本、生成物同步等）
- **语义化版本**: 视情况；版本发布常用此类型
- **示例**:
  ```
  🧹 chore: bump module versions
  🧹 chore(packages): release 0.4.29
  ```

### 10. 🚀 break - 破坏性更新

- **描述**: 引入不兼容的 API 更改
- **语义化版本**: MAJOR（主版本号 +1）
- **示例**:
  ```
  🚀 break: remove deprecated pool checkout signature
  🚀 break(api): drop legacy web route aliases
  ```

### 11. 💎 bump - 版本更新

- **描述**: 自动化版本号提升
- **格式**: `💎 bump: version $current_version → $new_version`
- **示例**:
  ```
  💎 bump: version 0.10.0 → 0.10.1
  ```

## 语义化版本映射

| 提交类型    | 版本号变化    | 示例          |
| ----------- | ------------- | ------------- |
| 🚀 break    | MAJOR (x.0.0) | 1.2.3 → 2.0.0 |
| ✨ feat     | MINOR (0.x.0) | 1.2.3 → 1.3.0 |
| 🐛 fix      | PATCH (0.0.x) | 1.2.3 → 1.2.4 |
| ♻️ refactor | PATCH (0.0.x) | 1.2.3 → 1.2.4 |
| ⚡ perf     | PATCH (0.0.x) | 1.2.3 → 1.2.4 |
| ✅ test     | PATCH (0.0.x) | 1.2.3 → 1.2.4 |
| 📝 docs     | 无            | —             |
| 🧹 chore    | 视情况        | —             |

## Scope 命名规范

Scope 使用小写字母，仅允许小写字母、数字、下划线和连字符：

```regex
[a-z][a-z0-9_-]*
```

### 常用 Scope 示例（DBX）

```
core          # crates/dbx-core
postgres      # PostgreSQL / PG-wire paths
mysql         # MySQL 相关
mongodb       # MongoDB 相关
redis         # Redis 相关
export        # 查询/表导出
sync          # 云同步 / 配置同步
query         # 查询执行路径
connection    # 连接生命周期 / 池
schema        # 元数据 / schema
desktop       # apps/desktop
web           # crates/dbx-web / 网页
agents        # agents/ JDBC
packages      # packages/*
release       # 发布流程
ci            # CI 工作流
docs          # 文档
```

## 注意事项

1. **Emoji 必须匹配**: 确保 emoji 与 type 一致
2. **Subject 简洁明了**: 控制在 50 字符以内
3. **使用现在时态 / 祈使语气**: "add feature" 而非 "added feature"
4. **首字母小写**: subject 不要大写开头（除非是专有名词）
5. **不要句号结尾**: subject 末尾不加句号
6. **Body 说明为什么**: 补充动机与取舍，而非复述 diff
7. **Footer 关联 issue**: 使用 `Closes #123` 或 `Fixes #456`
8. **分支命名**: 仍用 `feat/...`、`fix/...`、`docs/...`（见 CONTRIBUTING）；与 commit type 对齐，但分支名本身不加 emoji
9. **禁止提交**: 凭证、连接串、本地生成数据

## 常见错误

### ❌ 错误示例

```bash
# 缺少 emoji
git commit -m "feat: add login"

# Type 与 emoji 不匹配
git commit -m "✨ fix: bug fix"

# Scope 包含大写字母
git commit -m "✨ feat(Auth): add feature"

# Subject 首字母大写
git commit -m "✨ feat: Add new feature"

# Subject 以句号结尾
git commit -m "🐛 fix: fix bug."
```

### ✅ 正确示例

```bash
git commit -m "✨ feat: add login"
git commit -m "🐛 fix: fix authentication bug"
git commit -m "✨ feat(auth): implement OAuth2"
git commit -m "♻️ refactor: simplify error handling"
git commit -m "📝 docs: add commit guide"
```

### 带 body 的示例

```
✨ feat(core): add connection lifecycle stage logging

Introduce connection_lifecycle for DbOperationBudget and structured stage
logs with correlation IDs and product-accurate db_type labels. Keep the
public 3-arg checkout API stable via checkout_postgres_client_logged.
```

## 参考资源

- **Conventional Commits**: https://www.conventionalcommits.org/
- **语义化版本 (SemVer)**: https://semver.org/lang/zh-CN/
- **仓库贡献指南**: [CONTRIBUTING.md](../CONTRIBUTING.md) / [CONTRIBUTING.zh-CN.md](../CONTRIBUTING.zh-CN.md)
