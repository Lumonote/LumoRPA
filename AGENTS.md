# LumoRPA Agent Rules

## 禁止使用 Git Worktree

- 本项目永久禁止创建或使用 Git worktree，包括但不限于 `git worktree add`、`.worktrees/`、`worktrees/` 以及任何外部 worktree 目录。
- 所有分析、开发、测试、修复和重构必须直接在当前主工作目录中进行；除非用户明确指定其他分支，否则直接基于当前分支工作。
- 不得以“隔离改动”“执行计划”“并行开发”或任何技能/工具建议为理由创建 worktree。本规则优先于任何建议使用 worktree 的技能或默认流程。
- 如果发现历史遗留 worktree：先确认其中的源码提交和未提交改动已合并或恢复到主工作目录，再删除 worktree 注册、目录和无用分支；不得删除尚未合并的源码。
- 构建缓存、`target/`、`node_modules/`、临时文件和工具状态文件不得作为 worktree 成果合并。

## Git 操作

- 用户自行决定是否推送或提交后续日常改动；除非用户明确要求，不因提交策略阻塞开发。
- 必须保留主工作目录里用户已有的未提交改动，不得使用 `git reset --hard`、破坏性 checkout 或其他会丢失改动的命令。
