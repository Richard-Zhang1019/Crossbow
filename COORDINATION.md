# Agent 协作方案

同一台机器、同一 clone 的所有 git worktree 共用 Git 元数据目录。
临时协作状态存放在那里，仓库只保留稳定规则和脚本。**任何仓库改动都走 PR，禁止直接推送 main，包括协作文档。**

## 记录分工

| 记录 | 存放位置 | 用途 |
|---|---|---|
| 本机认领/完成流水 | `<git-common-dir>/agent-coordination/<会话名>.jsonl` | 开工前检查重叠，记录分支、worktree、issue、文件范围及结果 |
| 长期工作记录 | GitHub Issue / PR | 问题背景、实现、评审、验证与合并结果 |
| 协作规则与脚本 | `AGENTS.md`、本文件、`scripts/coordination.py` | 通过普通 PR 修改 |

## Worktree 布局

| 路径 | 会话 |
|---|---|
| /Users/ayu/Desktop/Crossbow | zcode |
| /Users/ayu/Crossbow-codex | codex |

不硬编码 `.git` 路径：linked worktree 的 `.git` 通常只是一个指针文件。
脚本使用 `git rev-parse --git-common-dir`，因此从任意 worktree、任意子目录运行都能找到同一份记录。
不同 clone、不同机器的记录不会同步；跨机器协作需另用 GitHub Issue 约定分工。

## 日常流程

需要 Python 3.8+ 和 Git，不需要第三方 Python 包。以下命令在自己的 worktree 根目录运行：

```sh
# 查看所有正在进行的任务；status --json 可输出机器可读结果
python3 scripts/coordination.py status

# 进入自己的工作分支、创建 issue 后认领；123/456 是示例编号
python3 scripts/coordination.py claim --session codex --issue 123 \
  --file apps/desktop/src/pages/Home --file apps/desktop/src/index.css \
  --summary "优化首页更新提示"

# 推送工作分支、创建 PR 后完成本次任务
python3 scripts/coordination.py finish --session codex --pr 456 --summary "已提交 PR，前端构建通过"

# 查看当前共享目录或某会话历史
python3 scripts/coordination.py path
python3 scripts/coordination.py history --session codex
```

`--file` 是仓库根目录相对路径，不接受通配符、仓库外路径或 Git 元数据路径。
路径可以尚不存在；目录覆盖其子树。重叠文件/目录、同一分支、同一 worktree、同一会话的未完成认领都会拒绝。
会话名使用小写字母、数字、连字符、下划线，以字母开头。多个并行会话不能共用 `codex` 这个名称。

## 并发与异常处理

- 所有脚本操作在共享目录通过原子创建 `.lock` 目录串行执行，认领检查与追加写入在同一次锁内完成，避免同时认领同一文件。
- 每个会话只向自己的 JSONL 文件追加事件；完成事件关联原 claim_id。`status` 从完整流水推导当前状态，不维护另一份易过期的快照。
- 完成操作必须来自原 worktree/分支；先完成，再切换分支。不会自动释放长期未完成的任务。
- 锁等待超过 5 秒会报错，不会自动抢锁。会话崩溃时，先核实 `.lock/owner.json` 记录的进程确实已结束，再人工清理该锁目录；不要删除流水或释放仍在工作的会话。
- 流水损坏会停止操作，不会静默忽略潜在认领。修复、恢复陈旧认领须由原会话或用户确认后进行。
- 这是一套本机协作约定，不是 Git 权限控制。脚本没有 commit/push 操作；若需从服务端强制禁止直推，可另行设置 GitHub 分支保护，本次不自动改变远端权限。

## 迁移与历史

旧 `coordination/` 已原样迁移到 `docs/coordination-history/`，只供追溯，不再追加。
旧记录中的“直推 main”规则已废止。临时状态保存在 Git 元数据内部，自然不进入 `git status`，无需增加 `.gitignore` 规则。

首次采用本方案时，各会话自行登记仍在进行的任务；**空状态不代表旧任务已经完成**。
不从历史文本自动推断或代替其他会话认领。仓库中 `AGENTS.md` 与本文件只在规则本身变化时通过 PR 修改，日常开工/收工不再修改仓库文件。

## 脚本验证

```sh
python3 -m unittest discover -s scripts/tests -v
```

测试使用临时仓库和真实 linked worktree，验证共享目录、并发竞争、目录重叠、追加式完成及工作区/分支不被修改。
