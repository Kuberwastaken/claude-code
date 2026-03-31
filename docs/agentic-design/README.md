# Claude Code Agentic 设计文档

> **目标读者**：有一定软件工程背景、希望通过真实项目理解 agentic 系统设计的学习者。
> **建议用时**：约 2.5 小时
> **阅读语言**：中文正文 + 英文技术术语

---

## 这个项目是什么？

[Claude Code](https://claude.ai/code) 是 Anthropic 开发的 AI 辅助编程 CLI 工具。它不是一个简单的"问答机器人"，而是一个完整的 **agentic system**：

- 它可以自主执行多轮工具调用（读文件、改代码、运行命令）
- 它可以 spawn 子 agent 并行处理复杂任务
- 它有 permission 管控、memory 管理、context 压缩等完整的 agent 基础设施

这套文档通过分析 Claude Code 的源代码，提炼出其中最有价值的 agentic 设计选择，帮助你建立对真实 agent 系统的直觉。

---

## 整体架构

```
┌─────────────────────────────────────────────────────────────────┐
│                         用户 / IDE / SDK                         │
└────────────────────────────┬────────────────────────────────────┘
                             │ 输入：用户消息
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                    CLI / Entrypoint Layer                        │
│  entrypoints/cli.tsx  │  entrypoints/sdk/  │  entrypoints/mcp/  │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Query Loop Engine                          │
│         QueryEngine.ts  ←→  query.ts  ←→  query/               │
│     (状态机: QUERY → TOOL_USE → RESULT → QUERY → ...)           │
└──────────────┬────────────────────────────────┬─────────────────┘
               │                                │
               ▼                                ▼
┌──────────────────────────┐    ┌───────────────────────────────┐
│    Claude API (Streaming) │    │      Tool Execution Layer     │
│  services/api/           │    │  services/tools/              │
│  • token budget 追踪      │    │  • StreamingToolExecutor      │
│  • compaction 触发        │    │  • 并发控制 + sibling abort   │
└──────────────────────────┘    └──────────────┬────────────────┘
                                               │
                            ┌──────────────────┼──────────────────┐
                            │                  │                  │
                            ▼                  ▼                  ▼
                     ┌─────────┐        ┌──────────┐      ┌──────────────┐
                     │  File   │        │  Bash /  │      │  AgentTool   │
                     │  Tools  │        │  Shell   │      │ (子 agent)   │
                     └─────────┘        └──────────┘      └──────┬───────┘
                                                                  │
                                         ┌────────────────────────┘
                                         │ spawn
                                         ▼
                              ┌────────────────────────┐
                              │   子 Agent / Coordinator│
                              │   coordinator/          │
                              │   tasks/Local|Remote    │
                              └────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                     横切关注点（Cross-cutting）                   │
│  Permission System  │  Context/Memory  │  Feature Gating        │
│  utils/permissions/ │  memdir/ autoDream│  bootstrap/state.ts   │
└─────────────────────────────────────────────────────────────────┘
```

---

## 核心设计哲学

**1. Tool 是 Agent 触碰世界的唯一手段**
Agent 的所有副作用（写文件、执行命令、发消息）都必须经过 Tool 接口，这使得权限控制、审计日志、测试 mock 都可以在一个地方统一处理。

**2. Text is the protocol**
Agent 之间通过 `<task-notification>` XML 消息通信，memory 以 Markdown 文件存储，permission rules 是 `Bash(git *)` 这样的字符串模式。用文本作为协议，意味着 AI 模型自己也可以读懂并调试这些通信内容。

**3. Failure modes are first-class**
每个长时间运行的操作都有明确的 abort 路径，例如：连续 3 次 permission denial 就回退到手动提示；并行 tool 中一个失败会触发 sibling abort；compaction 失败有 rollback 保护。

**4. Context window 是最稀缺的资源**
整个系统的大量设计决策（system prompt 的静态/动态分割、token budget 追踪、auto-dream 后台压缩）都围绕着"如何最大化利用有限的 context window"展开。

**5. 编译期与运行期特性隔离**
内部功能通过 Bun 的 `feature()` API 在编译时消除，不出现在公开二进制文件中；运行时行为通过 GrowthBook runtime flags 动态控制。

---

## 建议阅读顺序

| 编号 | 文档 | 用时 | 核心问题 |
|------|------|------|---------|
| [00](./00-codebase-tour.md) | 代码库目录全景 | 30 min | "这个 repo 里有什么？" |
| [01](./01-agent-loop.md) | 核心 Query Loop | 20 min | "Agent 是如何循环运作的？" |
| [02](./02-tool-system.md) | Tool 系统 | 20 min | "Tool 是怎么被定义和执行的？" |
| [03](./03-multi-agent-coordination.md) | 多 Agent 协调 | 25 min | "多个 agent 怎么协作？" |
| [04](./04-permission-system.md) | Permission 系统 | 20 min | "Agent 怎么知道自己能做什么？" |
| [05](./05-context-and-memory.md) | Context 与 Memory | 20 min | "Context window 满了怎么办？" |
| [06](./06-feature-gating.md) | Feature Flag 系统 | 10 min | "内部功能是怎么隐藏的？" |

---

## 如何使用这套文档

- **按顺序读**：00 → 01 → 02 → 03 → 04 → 05 → 06，每篇都假设你已读过前面的内容
- **跳读**：如果你已熟悉某个概念，可以直接跳到感兴趣的章节
- **对照代码**：每篇文档都标注了关键源文件路径，随时可以打开对照阅读
- **关注 Design Decision 专栏**：这些是最有学习价值的地方，解释了"为什么这样设计"而不只是"是什么"
