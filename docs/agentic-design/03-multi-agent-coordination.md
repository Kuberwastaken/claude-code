# 多 Agent 协调：并行与分解

## 为什么需要多个 Agent?

有些任务太复杂，一个 agent 力不从心：

- 用户说："帮我把代码库从 TypeScript 迁到 Rust"
- 一个 agent 从第一个文件开始，改 5 个小时后，context 满了

更好的方式：**分解任务**

- Coordinator agent 解析任务、制定计划
- Worker agent 1 并行处理"转换 utils 模块"
- Worker agent 2 并行处理"转换 API 层"
- Worker agent 3 并行处理"转换 UI 组件"
- Coordinator 总结结果、验证一致性

这就是 **多 agent 协调**。

---

## 核心概念：Agent 类型

Claude Code 支持多种 agent：

### 1. Local Agent（进程内）

```typescript
// 在当前进程中 spawn 一个子 agent
const subAgent = await spawnAgent({
  type: "local",
  agentDef: generalPurposeAgent,
  initialMessage: "帮我测试这个函数"
})
```

特点：
- 共享内存（同一个 Node.js 进程）
- 上下文隔离（通过 `AsyncLocalStorage`）
- 无网络开销
- 最快的交互

### 2. Remote Agent（云容器）

```typescript
const remoteAgent = await spawnAgent({
  type: "remote",
  agentDef: exploreAgent,
  initialMessage: "搜索 codebase 中的 bug"
})
```

特点：
- 在 CCR（Cloud Container Runtime）中运行
- 独立的资源池
- 支持长时间运行（最多 30 分钟）
- 网络传输延迟，但隔离度最高

### 3. Forked Agent（进程派生）

```typescript
const forkedAgent = spawnForkedAgent({
  agentScript: "./my-agent.ts",
  args: ["--mode", "debug"]
})
```

特点：
- 创建新的 Node.js 子进程
- 完全隔离的 V8 引擎
- 支持 CPU 密集任务（不会阻塞主线程）

### 4. In-Process Teammate（进程内同伴）

```typescript
const teammate = await createTeam({
  name: "research-squad",
  agents: [agent1, agent2, agent3],
  strategy: "parallel"
})
```

特点：
- 多个 agent 同时运行
- 共享邮箱（互相发消息）
- 同步协调，不需要网络

---

## Agent 的 Spawning 和隔离

### 怎样 Spawn 一个 Agent?

通过 `AgentTool` (文件：`tools/AgentTool/AgentTool.tsx`)：

```typescript
// Claude 调用 AgentTool
Agent: "让我 spawn 一个 explore agent 来搜索代码库"

Tool Call:
  name: "AgentTool"
  input: {
    type: "spawn",
    agentDef: "exploreAgent",
    prompt: "搜索 utils/ 下的性能问题",
    isolationMode: "local"
  }

返回:
  agentId: "abc123"
  status: "running"
```

### 上下文隔离：`AsyncLocalStorage`

为什么需要隔离？子 agent 的全局状态不应该污染父 agent：

```typescript
// 主 agent 的全局状态
const sessionId = "parent-session-id"
const permissions = "default"

// 在子 agent 中，这些应该被**覆盖**
asyncLocalStorage.run({
  sessionId: "child-agent-id",
  permissions: "bubble",  // 特殊权限给 worker
  memoryDir: "/tmp/child-memory"
}, () => {
  runChildAgent()  // 子 agent 看到的是覆盖后的值
})
```

实现：`AsyncLocalStorage` 是 Node.js 内置的上下文隔离机制。

---

## Coordinator 模式：4 个阶段

启用 `CLAUDE_CODE_COORDINATOR_MODE=1` 时，agent 进入 coordinator 模式。

### 阶段结构

```
用户请求: "帮我重构这个 monorepo"
  ↓
[1] RESEARCH PHASE
    Coordinator 说:"我需要理解项目结构"
    Worker 1 探索 package.json + tsconfig
    Worker 2 探索 src/ 目录结构
    Worker 3 探索 dependencies 图
    → 输出: 项目现状报告
  ↓
[2] SYNTHESIS PHASE
    Coordinator 读 3 个 worker 的报告
    Coordinator 说："基于这些发现，我的计划是..."
    → 输出: 详细重构计划
  ↓
[3] IMPLEMENTATION PHASE
    Coordinator 把计划分解为任务
    Worker A 修改 package.json
    Worker B 更新 tsconfig
    Worker C 调整 src/ 组织
    → 并行执行
  ↓
[4] VERIFICATION PHASE
    Coordinator 说："让我验证改动"
    Worker 运行测试
    Worker 检查类型错误
    → 输出: 验证报告
  ↓
完成：Coordinator 总结全流程，返回最终结果
```

### 通信机制：`<task-notification>`

Worker 通过 XML 消息向 Coordinator 报告进度：

```xml
<task-notification>
  <task-id>research-1</task-id>
  <status>complete</status>
  <summary>Found 3 monorepo packages</summary>
  <details>
    - Package A: React components
    - Package B: Utils library
    - Package C: CLI tool
  </details>
</task-notification>
```

Coordinator 看到这个消息，就知道：
- Task "research-1" 完成了
- 发现了 3 个包

相关文件：`coordinator/coordinatorMode.ts`（包含完整的 system prompt，大约 330 行）

---

## 共享 Scratchpad

Worker 可以在共享空间写临时数据（feature gate: `tengu_scratch`）：

```typescript
// Worker 1 写入
await writeToScratchpad("project_structure.md", """
# Monorepo Structure
- packages/ui/
  - components/
  - styles/
""")

// Worker 2 读取
const structure = await readFromScratchpad("project_structure.md")

// Coordinator 读取
const allNotes = await listScratchpad()
```

优点：
- Worker 不需要把所有信息放在 user 消息中
- 大文件可以直接读取，不占用 context window
- 自然的"工作台"抽象

---

## Agent 内存和记忆

每个 agent 可以有自己的 memory：

```
~/.claude/memory/
├── team-memory/           # 团队共享
│   ├── shared-findings.md
│   └── progress.md
├── agent-abc123/          # Agent 特定
│   ├── MEMORY.md          # Agent 的长期知识库
│   └── session-transcript.log
```

**Team Memory Sync** (`services/teamMemorySync/`) 确保：
- 所有 agent 可以读取共享的 findings
- 不会产生数据竞争（使用 file lock）

---

## Design Decision 专栏

### 为什么用 `<task-notification>` XML 而不是结构化 IPC?

不好的设计（二进制 IPC）：
```
Worker 发送: MessageType::RESEARCH_COMPLETE {
  task_id: 0x123,
  status: 0x02,
  payload: [0xAB, 0xCD, ...]
}
```

问题：
- Claude 看不懂二进制协议
- 如果需要调试，你必须手动反序列化
- 扩展协议时需要修改版本号

更好的设计（XML）：
```
<task-notification>
  <task-id>abc123</task-id>
  <status>complete</status>
  <summary>Found 100 bugs</summary>
</task-notification>
```

优点：
- Claude 可以**直接阅读和生成**这种格式
- 调试时在日志中直接看到可读的消息
- 自文档化（XML 标签说明意义）
- 易于扩展（添加新字段不破坏旧系统）

**Text is the protocol** 的又一例证。

### 为什么 Coordinator 要等 Research 完全结束才进行 Synthesis?

如果不等（流式处理）：
```
Worker 1 报告 → Coordinator 开始 synthesis
Worker 2 报告（晚到）→ Coordinator 需要重新 synthesis
Worker 3 报告（更晚）→ 又得重新来一遍
```

问题：浪费 token 和时间在重复的 synthesis。

如果等待（阶段隔离）：
```
所有 Worker 完成 Research
  ↓
Coordinator 一次性读完所有报告
  ↓
一次 Synthesis，不需要改
```

权衡：
- 阶段隔离：时间线性（4 个阶段顺序执行）
- 流式处理：时间可能更短（如果 worker 速度差异大），但 prompt 可能更长

Claude Code 选择阶段隔离，因为系统提示清晰度更重要。

---

## 常见误解

**误解 1**："Coordinator 也会执行 tool?"

实际：Coordinator **只** spawn worker 和管理任务。所有的 tool 执行（读文件、改代码、运行命令）都在 worker 中进行。Coordinator 只是看消息、做决策、发指令。

**误解 2**："Worker 之间可以直接通信?"

实际：Worker 通过 **Coordinator 中转**。这样 Coordinator 可以：
- 理解全局状态
- 决定优先级
- 在冲突时仲裁

直接 worker-to-worker 通信会导致难以追踪的依赖和死锁。

**误解 3**："多个 agent 肯定比单个快?"

实际：取决于任务：
- **可并行化**（如搜索 3 个不同目录）：并行快
- **高度依赖**（后续任务需要前序结果）：并行反而慢（overhead）

Claude Code 的 Coordinator 会选择合适的分解策略。

---

## 关键要点

1. **Agent 类型**：local (in-process), remote (CCR 30min), forked, teammate
2. **上下文隔离**：通过 `AsyncLocalStorage` 分离全局状态
3. **Coordinator 模式**：4 个阶段（research → synthesis → implementation → verification）
4. **Worker 通信**：通过 `<task-notification>` XML，Coordinator 中转
5. **Shared scratchpad**：大文件存储，不占 context window

---

## 深入阅读

- `tools/AgentTool/AgentTool.tsx`：Agent spawning 逻辑
- `tools/AgentTool/forkSubagent.ts`：Fork 实现
- `coordinator/coordinatorMode.ts`：Coordinator system prompt 和阶段逻辑
- `services/teamMemorySync/`：Team memory 同步

下一步：了解**权限系统**，看看 Agent 怎么知道"我可以执行什么 tool"。
