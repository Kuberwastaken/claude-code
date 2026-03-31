# 代码库全景：从顶层到最深处

> 这篇文档是一个**递归的导游**。我们从顶层文件开始，逐层深入每个重要的目录，直到粒度太细而没有学习价值为止。每个章节都注明关键文件，你可以随时打开源代码对照。

---

## 顶层：核心文件

在 `/home/user/claude-code/` 的根目录下，以下文件是系统的脊梁：

| 文件 | 行数 | 用途 |
|------|------|------|
| `main.tsx` | ~4700 | CLI 入口，React + Ink 终端渲染器 |
| `query.ts` | ~1700 | Query loop 状态机的完整实现 |
| `QueryEngine.ts` | 核心 | Query 执行的编排引擎 |
| `Tool.ts` | 核心 | Tool 的类型定义和接口规范 |
| `tools.ts` | 核心 | Tool 注册与初始化 |
| `context.ts` | 核心 | System prompt 和 context 的组装 |
| `commands.ts` | ~700 | 所有 slash command 的中央路由 |

**学习路线**：`main.tsx` → `query.ts` → `Tool.ts` 可以了解 agent loop 的完整流程。

---

## `entrypoints/` — 入口层

系统支持多种启动方式：

| 子目录/文件 | 用途 |
|-----------|------|
| `cli.tsx` | 标准 CLI 入口，React 组件树的根，feature detection |
| `sdk/` | Anthropic SDK 集成（给程序员用） |
| `mcp.ts` | MCP (Model Context Protocol) 服务器 |
| `init.ts` | 初始化逻辑，首次运行配置 |
| `sandboxTypes.ts` | 沙箱相关的类型定义 |

**关键点**：`cli.tsx` 做 feature flag 检测，决定如何初始化全局状态。

---

## `query/` — Query Loop 配置和支撑

这个目录下的文件都是 `query.ts` 的辅助组件：

| 文件 | 用途 |
|------|------|
| `config.ts` | Query 配置构建器，参数组合 |
| `tokenBudget.ts` | Token budget 追踪和强制执行 |
| `stopHooks.ts` | Stop/interrupt 处理 |
| `deps.ts` | Query 依赖注入 |

**最重要的**：`tokenBudget.ts` 控制着"超限时触发 compaction"的逻辑。

---

## `tools/` — 所有 Tool 实现（40+ 个）

这个目录有 40 多个子目录，每个代表一个 Tool。核心的几个：

### 必读：

| Tool | 文件 | 用途 |
|------|------|------|
| `AgentTool/` | 子 agent 的产卵和管理 | 核心，递归往下 |
| `FileReadTool/` | 读文件 | 基础 |
| `FileEditTool/` | 编辑文件 | 基础 |
| `FileWriteTool/` | 写文件 | 基础 |
| `BashTool/` | 执行 shell 命令 | 基础 |
| `GlobTool/` | 文件匹配 | 基础 |
| `GrepTool/` | 内容搜索 | 基础 |
| `WebFetchTool/` | 获取网页 | 基础 |
| `AskUserQuestionTool/` | 询问用户 | 权限边界 |
| `TodoWriteTool/` | 写 todo list | 后台任务 |

### `AgentTool/` — 深递归

这是最复杂的 Tool，用来 spawn 子 agent：

```
AgentTool/
├── AgentTool.tsx          [233 KB] 主逻辑，agent 生命周期
├── runAgent.ts            执行 agent 的主函数
├── forkSubagent.ts        fork 一个新 agent
├── resumeAgent.ts         恢复之前的 agent
├── loadAgentsDir.ts       从 ~/.agents/ 加载 agent 定义
├── built-in/              内置 agent
│   ├── generalPurposeAgent.ts
│   ├── planAgent.ts
│   ├── exploreAgent.ts
│   └── ... (其他 5 个)
├── agentMemory.ts         Agent 记忆快照
├── agentColorManager.ts   Agent 的颜色分配（便于输出区分）
└── ... (其他支撑文件)
```

**关键概念**：Agent 可以是 local (in-process) 或 remote (CCR)，使用 `AsyncLocalStorage` 隔离上下文。

---

## `services/` — 业务逻辑服务

这个目录是 Claude Code 的"心脏"：

### `tools/` — Tool 执行编排

```
services/tools/
├── StreamingToolExecutor.ts    [核心] 并发 tool 执行，结果缓冲
├── toolOrchestration.ts        Tool 调度和权限检查
├── toolExecution.ts            单个 tool 调用的执行
├── toolHooks.ts                Tool 执行前/后的 hook
└── ...
```

**最重要**：`StreamingToolExecutor.ts` 实现了"并发执行但顺序输出结果"的逻辑。

### `autoDream/` — 自动记忆压缩

```
services/autoDream/
├── autoDream.ts                [11 KB] Dream 流程：3 个触发门 + 4 个阶段
├── consolidationPrompt.ts      [核心] 告诉 Claude 怎么压缩 memory
├── consolidationLock.ts        防并发的 lock 机制
└── config.ts                   Dream 参数配置
```

**关键点**：Dream 有三个触发条件门：24h 时间 + 5+ 个 session + 拿到 lock。

### 其他重要服务

| 目录 | 用途 |
|------|------|
| `api/` | Claude API 调用，token 计算，usage 追踪 |
| `mcp/` | MCP 协议服务器的实现 |
| `analytics/` | 事件上报、GrowthBook feature flags |
| `plugins/` | 插件系统 |

---

## `utils/permissions/` — 权限系统（深递归）

这是整个 agent sandbox 的防线，共 20+ 个文件，总计 300+ KB：

```
utils/permissions/
├── permissions.ts              [52 KB] 核心决策逻辑
├── yoloClassifier.ts           [52 KB] ML 自动审批分类器
├── filesystem.ts               [62 KB] 文件系统安全规则
├── PermissionMode.ts           Permission mode 枚举
├── denialTracking.ts           追踪拒绝次数，circuit breaker
├── pathValidation.ts           路径安全（Unicode normalization 等）
├── permissionRuleParser.ts     解析 "Bash(git *)" 这样的规则
├── classifierDecision.ts       分类器决策包装
├── bashClassifier.ts           Bash 命令的特殊规则
├── permissionExplainer.ts      用 LLM 解释权限决定
└── ... (其他辅助)
```

**核心 pipeline**：mode → rules → YOLO classifier → prompt user

---

## `utils/` — 其他公用工具（部分列举）

```
utils/
├── permissions/                [见上面的深递归]
├── messages/
│   ├── mappers.ts             SDK 消息格式转换
│   ├── systemInit.ts          系统初始化消息构建
│   └── ...
├── settings/                   设置加载和应用
├── queryHelpers.ts             Query 辅助函数
├── undercover.ts               隐藏内部代号（Capybara, Tengu 等）
├── abortController.ts          Abort signal 管理
├── agentContext.ts             Agent 上下文 getter
├── api.ts                       API 工具函数
├── hooks/                       React hooks （不是本目录）
├── ... (数十个其他)
```

**学习重点**：`permissions/` 最复杂，其次是 `messages/` 和 `settings/`。

---

## `coordinator/` — 多 Agent 协调

```
coordinator/
└── coordinatorMode.ts          [19 KB] 唯一的文件，包含完整 coordinator 系统提示

功能：启用 CLAUDE_CODE_COORDINATOR_MODE=1 时激活，引入 4 个阶段：
  research → synthesis → implementation → verification
```

**关键点**：Worker 通过 `<task-notification>` XML 向 coordinator 报告进度。

---

## `tasks/` — 后台任务管理

```
tasks/
├── LocalAgentTask/             本地 agent 任务追踪
├── RemoteAgentTask/            远程 agent 任务追踪（CCR）
├── LocalMainSessionTask.ts     主 session 的任务
├── LocalShellTask/             Shell 任务（tmux/iTerm2）
├── InProcessTeammateTask/      In-process teammate 任务
├── DreamTask/                  Memory dream 任务
├── stopTask.ts                 停止任务的逻辑
├── types.ts                    Task 的类型定义
└── pillLabel.ts                任务标签（UI 展示）
```

**核心概念**：每个后台任务都有生命周期追踪。

---

## `state/` — React 状态管理

```
state/
├── AppState.tsx                React 状态类型定义
├── AppStateStore.ts            [21 KB] Zustand-like 状态存储
├── onChangeAppState.ts         状态变化的 handler
├── selectors.ts                状态选择器
├── store.ts                    存储初始化
└── teammateViewHelpers.ts      Teammate 视图辅助
```

**模式**：这是 Claude Code 的全局状态树，UI 所有数据都来自这里。

---

## `hooks/` — 80+ 个 React Hook

重要的：

| Hook | 用途 |
|------|------|
| `useCanUseTool.tsx` | [40 KB] 权限检查 Hook（频繁调用） |
| `useGlobalKeybindings.tsx` | [31 KB] 全局键盘快捷键 |
| `useTypeahead.tsx` | [212 KB] 自动补全建议 |
| `useVoice.ts` | [45 KB] 语音输入集成 |
| `useInboxPoller.ts` | [34 KB] 后台轮询消息 |
| `useArrowKeyHistory.tsx` | [34 KB] 历史导航（上/下箭头） |
| `useReplBridge.ts` | [115 KB] REPL 集成 |

**学习策略**：跳过大部分 hook，只深入 `useCanUseTool` 和 `useTypeahead`。

---

## `bootstrap/state.ts` — 全局启动状态

```
bootstrap/
└── state.ts                    [56 KB]
```

内容：
- 当前 session ID 和持久化 flag
- 用户类型（ant/public）
- Feature flag 缓存
- KAIROS mode 检测
- 全局配置状态

**作用**：在 App 启动的最早时刻初始化所有全局状态。

---

## `cli/` — 终端输出和格式化

```
cli/
├── print.ts                    [212 KB] 终端渲染引擎
├── structuredIO.ts            NDJSON 和结构化输出
├── handlers/                   命令特定的输出 handler
├── transports/                 多种输出后端（stdout, HTTP 等）
├── remoteIO.ts                远程 IO
└── ... (其他)
```

**关键**：`print.ts` 是整个 Claude Code 的"输出黑洞"，所有文本最终都通过这里。

---

## `context/` — React Context Provider

```
context/
├── QueuedMessageContext.tsx    消息队列
├── mailbox.tsx                Agent 邮箱（inter-agent 消息）
├── modalContext.tsx            Modal 对话框
├── notifications.tsx           通知系统
├── overlayContext.tsx          浮层上下文
├── promptOverlayContext.tsx    提示浮层
├── stats.tsx                   统计数据
└── voice.tsx                   语音上下文
```

**作用**：提供 React component tree 范围内的全局上下文。

---

## `memdir/` — 用户 Memory 管理

```
memdir/
├── memdir.ts                   [21 KB] 核心 memory 目录管理
├── memoryScan.ts              扫描 memory 文件
├── memoryTypes.ts             Memory 文件的类型
├── memoryAge.ts               Memory 的年龄计算
├── paths.ts                    Memory 文件路径管理
├── teamMemPaths.ts            Team memory 路径（multi-agent）
└── findRelevantMemories.ts    查找相关 memory
```

**用途**：管理 `~/.claude/memory/` 目录，存储用户的持久化知识。

---

## `constant/` — 常量和系统提示

```
constant/ (也被称为 constants/)
├── system.ts                   Base system prompt
├── systemPromptSections.ts    System prompt 的可组装部分
├── cyberRiskInstruction.ts    安全指导（Safeguards team 维护）
├── betas.ts                    API beta features 列表
├── messages.ts                 常见消息文本
├── prompts.ts                  各种 prompt 模板
└── ... (其他常量)
```

**关键**：System prompt 是模块化的，不同 feature 会添加不同的部分。

---

## `bridge/` — 与 claude.ai 的 JWT 连接

```
bridge/
├── bridgeApi.ts               与 claude.ai 的通信
├── bridgeMessaging.ts         消息协议
├── remoteBridgeCore.ts        远程 bridge 核心
├── createSession.ts           创建远程 session
├── jwtUtils.ts                JWT 令牌工具
├── trustedDevice.ts           受信设备管理
└── ... (10+ 个其他)
```

**用途**：BRIDGE_MODE 时，CLI 可以通过 claude.ai 的 WebUI 远程控制。

---

## `remote/` — 远程 Session 管理

```
remote/
├── RemoteSessionManager.ts    远程 session 生命周期
├── SessionsWebSocket.ts       WebSocket 连接管理
├── remotePermissionBridge.ts  权限同步
├── sdkMessageAdapter.ts       SDK 消息适配
└── ... (其他)
```

**用途**：支持多个远程客户端同时连接到同一个服务。

---

## `entrypoints/sdk/` — SDK 集成

```
entrypoints/sdk/
├── agentSdkTypes.ts           给外部程序使用的类型
└── (其他 SDK 特定代码)
```

**用途**：让程序员可以用代码调用 Claude Code，而不只是 CLI。

---

## `components/` — React UI 组件

```
components/
├── App.tsx                     顶层 app 组件
├── AgentProgressLine.tsx       Agent 进度显示
├── BaseTextInput.tsx           文本输入基础
├── ... (100+ 个其他组件)
```

**特点**：使用 Ink 库在终端中渲染 React 组件。

---

## `commands/` — Slash 命令（88 个）

```
commands/
├── commit.ts / commit-push-pr.ts    Git 相关命令
├── agent.ts                         Agent 管理命令
├── task.ts, tasks.ts                任务管理
├── config.ts                        配置命令
├── memory.ts, memory-dream.ts       Memory 管理
├── session.ts                       Session 管理
├── autofix-pr.ts                    PR 自动修复
├── ... (80+ 个其他)
```

**特点**：每个 command 都可以定制输出格式，支持 structured output (NDJSON)。

---

## 递归粒度停止条件

以下类型的文件不再递归深入：

- **单功能工具函数** (<100 行，做一件明确的事)：如 `array.ts`, `api.ts` 的某些导出
- **简单 enum/type 定义**：如 `types.ts`, `*.d.ts`
- **单个小的 React Hook**：如 `useAfterFirstRender.ts`
- **Utility 映射和适配器**：如 message format converter

这些文件的学习价值在于"知道它存在"，而不是理解其内部细节。

---

## 学习路线建议

1. **快速扫描**：从 README 开始，看一眼这个全景。
2. **焦点学习**：根据你的兴趣选择深入：
   - 想理解 agent loop? → `query.ts`, `QueryEngine.ts`
   - 想理解 tool 系统? → `Tool.ts`, `tools/`, `services/tools/`
   - 想理解权限? → `utils/permissions/`
   - 想理解多 agent? → `tools/AgentTool/`, `coordinator/`
3. **对照原文**：打开相应的源文件，参照本文的路线图逐个浏览。

---

## 核心数据流

```
User Input
    ↓
entrypoints/cli.tsx (React 初始化)
    ↓
query.ts (Query Loop: QUERY → TOOL_USE → RESULT)
    ↓
Tool.ts & services/tools/ (Tool 执行)
    ↓
utils/permissions/ (权限检查) + BashTool/FileEditTool/... (实际执行)
    ↓
queryHelpers + messages (结果处理)
    ↓
state/ (状态更新)
    ↓
components/ + cli/print.ts (UI 渲染)
    ↓
User sees output
```

---

## 关键文件大小排名

```
Top 15 largest files (最值得读):

1. query.ts                 ~1700 行
2. services/tools/StreamingToolExecutor.ts
3. utils/permissions/permissions.ts    ~52 KB
4. utils/permissions/yoloClassifier.ts ~52 KB
5. utils/permissions/filesystem.ts     ~62 KB
6. hooks/useTypeahead.tsx             ~212 KB
7. cli/print.ts                       ~212 KB
8. tools/AgentTool/AgentTool.tsx      ~233 KB
9. bootstrap/state.ts                 ~56 KB
10. state/AppStateStore.ts             ~21 KB
11. services/autoDream/autoDream.ts    ~11 KB
12. coordinator/coordinatorMode.ts     ~19 KB
13. hooks/useCanUseTool.tsx            ~40 KB
14. hooks/useGlobalKeybindings.tsx     ~31 KB
15. hooks/useInboxPoller.ts            ~34 KB
```

先读前 5 个，会 cover 80% 的系统复杂性。
