# Tool 系统：Agent 影响世界的手段

## 为什么需要 Tool?

如果 Claude 只能回复文字，它就只是个聊天机器人。要让 Claude **执行动作**（读文件、改代码、运行命令），需要一个 **Tool 接口**。

Tool 系统解决的问题：
1. 如何告诉 Claude "有哪些动作可用"？
2. 如何验证 Claude 的请求是否合法（权限检查）？
3. 如何让多个 tool 并发执行，但结果保持有序？
4. 如何在 tool 执行失败时恢复？

---

## 核心概念：Tool Interface

一个 `Tool` 必须满足这个接口（`Tool.ts`）：

```typescript
interface Tool {
  // 1. 模型识别 tool 的名称
  name: string  // 如 "FileReadTool"

  // 2. 模型理解 tool 的用途（出现在 system prompt 中）
  description: string

  // 3. 模型生成的参数必须符合这个 schema
  inputSchema: JSONSchema  // 如 { type: "object", properties: { path: ... } }

  // 4. 执行 tool 的函数
  call(input: ToolInput): AsyncGenerator<ToolResult>

  // 5. 指示这个 tool 是否"安全"（后文详述）
  isSafe?: boolean

  // 6. 内部使用，不会发给模型
  internalMetadata?: { ... }
}
```

每个字段的意义：

### `name` 和 `description`
这两个被送到 Claude API，出现在 system prompt 的 tool 列表中：

```
Available tools:

1. FileReadTool
   Read the contents of a file from the filesystem
   Parameters: { path: string }

2. BashTool
   Execute a bash command in the user's shell
   Parameters: { command: string }
```

Claude 看到这个列表，学会了"我可以用 FileReadTool 来读文件"。

### `inputSchema`
这是 JSON Schema 格式，例如：

```typescript
{
  type: "object",
  properties: {
    path: {
      type: "string",
      description: "Absolute file path"
    }
  },
  required: ["path"]
}
```

Claude 会**严格遵守这个 schema**（模型已训练）。例如，如果你要求 `path` 必须是字符串，Claude 就不会发来 `{path: 123}`。

### `call(input)`
这是 tool 的实现。它返回一个 `AsyncGenerator`，每次 `yield` 都是一个结果。例如：

```typescript
async *call(input: { path: string }) {
  // 逐步产生结果（支持 streaming）
  yield { type: "start", message: "Opening file..." }
  yield { type: "content", data: file_content }
  yield { type: "done", lines_read: 100 }
}
```

为什么是 generator？因为某些 tool（如 BashTool）的输出可能很长，需要**流式返回**。

### `isSafe` 和权限

某些 tool 被标记为 `unsafe`：
- `BashTool` ← 可以执行任意命令，危险
- `FileEditTool` ← 可以修改文件，需要权限检查
- `FileReadTool` ← 只读，相对安全

Unsafe tool 在执行前**必须通过权限检查**。

---

## Tool 注册与过滤

在 `tools.ts` 中，所有 tool 被注册到一个中央注册表：

```typescript
const ALL_TOOLS: Tool[] = [
  AgentTool,
  BashTool,
  FileReadTool,
  FileEditTool,
  ...,
]
```

然后根据用户权限和配置进行**过滤**：

```typescript
// 仅公开 tool
const PUBLIC_TOOLS = ALL_TOOLS.filter(t => !t.internalOnly)

// 仅 ant 用户可用
const ANT_ONLY_TOOLS = [REPL_TOOL, CONFIGTOOL, ...]
```

这样，模型只会看到当前环境允许的 tool 清单。

---

## Tool 执行 Pipeline

从"模型调用 tool"到"返回结果"的完整流程：

```
Query Loop 收到 tool_use block
  ├─ Tool 名: "FileEditTool", Input: {path: "...", newContent: "..."}
  │
  ├─ 1. 验证 Tool 存在 ✓
  │
  ├─ 2. 权限检查 (utils/permissions/)
  │     - Mode: auto 还是 default?
  │     - 规则匹配? (如 "FileEdit(/src/*.ts)")
  │     - YOLO 分类器? (是否 auto-approve)
  │     - 如果拒绝 → 返回 permission_denied error
  │
  ├─ 3. 执行 Tool
  │     - 调用 tool.call(input)
  │     - 得到 AsyncGenerator
  │     - 逐个 yield 结果
  │
  ├─ 4. 收集结果
  │     - 内容可能很长，缓冲到内存
  │     - 或流式返回给 UI
  │
  └─ 5. 发送给 Claude
        - 组合成 user 消息: "Tool result: ..."
        - 继续 Query Loop
```

相关文件：
- 权限检查：`utils/permissions/permissions.ts`
- 执行编排：`services/tools/toolOrchestration.ts`
- 流式执行：`services/tools/StreamingToolExecutor.ts`

---

## 并发执行：多个 Tool 同时运行

一条 Claude 消息中可能包含多个 `tool_use` block：

```
Claude:
  1. 读文件 A (tool_use id=1)
  2. 读文件 B (tool_use id=2)
  3. 读文件 C (tool_use id=3)
```

这三个 tool 可以**并发执行**，但有几个复杂性：

### Concurrency Gate

同时最多 5 个 tool 运行（可配置）：

```typescript
const CONCURRENCY_LIMIT = 5

// 队列管理
const queue = [tool_1, tool_2, tool_3, ...]
while (queue.length > 0) {
  const batch = queue.splice(0, CONCURRENCY_LIMIT)
  const results = await Promise.all(
    batch.map(tool => tool.call(...))
  )
  // 处理结果
}
```

### In-Order Emission

虽然执行可能乱序，但**结果必须按调用顺序返回**：

```
调用顺序: tool_1, tool_2, tool_3
执行时序:          tool_2 ✓ (快速)
        tool_1 ✓ (较慢)
                 tool_3 ✓

返回顺序: tool_1 结果 → tool_2 结果 → tool_3 结果

为什么? 因为 Claude 期望看到它调用的顺序被保留
```

实现方式：使用**结果缓冲**：

```typescript
const results = new Map()  // id → result
let nextIdToEmit = 0

for each completed tool {
  results.set(tool.id, tool.result)

  // 检查是否可以按顺序 emit
  while (results.has(nextIdToEmit)) {
    yield results.get(nextIdToEmit)
    nextIdToEmit++
  }
}
```

### Sibling Abort

如果一个 tool 失败了，它的**兄弟 tool 会被立即取消**：

```
tool_1 → 运行中
tool_2 → 运行中
tool_3 → 运行中

tool_2 抛异常 ✗
  ↓
立即 abort tool_1 和 tool_3
  ↓
返回错误，整个批次失败
```

这是一个**快速失败**策略：不要继续浪费时间在其他 tool 上，直接告诉 Claude 出了问题。

相关文件：`services/tools/StreamingToolExecutor.ts`

---

## Safe vs Unsafe Tool Classification

Tool 分两类：

### Safe Tool（自动执行）
```
FileReadTool ← 只读，没有副作用
WebFetchTool ← 只是下载网页
GlobTool ← 只是列文件
GrepTool ← 只是搜索内容
```

这些可以在 `auto` permission mode 下自动执行，不需要用户确认。

### Unsafe Tool（需要权限检查）
```
BashTool ← 可能执行任意命令
FileEditTool ← 可能修改重要文件
FileWriteTool ← 可能覆盖数据
AgentTool ← 可能 spawn 新 agent
TaskCreateTool ← 可能创建后台任务
```

这些在执行前必须通过 `utils/permissions/` 的检查。

---

## Design Decision 专栏

### 为什么 Tool 返回 `AsyncGenerator` 而不是 `Promise<string>`?

不好的设计：
```typescript
call(input): Promise<string> {
  // 必须等待整个操作完成后才返回
  const result = await readFile(input.path)
  return result  // 10 MB 的代码文件？等吧
}
```

问题：
- 大文件时，UI 一直卡住，看不到进度
- Claude 看不到中间步骤

更好的设计：
```typescript
call(input): AsyncGenerator<ToolResult> {
  yield { type: "progress", message: "Reading..." }
  const content = await readFile(input.path)
  yield { type: "content", data: content }  // 分块返回
  yield { type: "complete", lines: 100 }
}
```

优点：
- UI 可以**立即显示**进度消息
- 长操作时用户看得到"不是卡住了，是在处理"
- 便于 debug：中间消息可以被记录

Generator 把**长时间操作的透明度**提升为一级特性。

### 为什么要保持结果顺序?

如果乱序返回：
```
Claude 说：读 A, 读 B, 读 C
我们返回：B 的结果, C 的结果, A 的结果
```

Claude 会困惑："我要的 A 呢？"

所以必须：
```
Claude 说：读 A, 读 B, 读 C
我们返回：A 的结果, B 的结果, C 的结果
```

这样 Claude 可以正确关联"我的第一个请求的响应是这个"。

---

## 常见误解

**误解 1**："Tool 就是函数调用？"

实际：Tool 是一个**完整的接口**，包括：
- 模型可见的名称和描述
- 输入规范（schema）
- 权限检查
- 执行编排
- Streaming 支持

函数调用只是内部实现的一部分。

**误解 2**："Tool 是 OpenAI function calling 的翻版？"

实际：Claude Code 的 Tool 设计更深层：
- 对权限的原生支持（OpenAI 需要自己实现）
- 对并发执行的编排
- 对长时间操作的 streaming 支持
- 更细粒度的安全分类

**误解 3**："所有 tool 都必须是同步的？"

实际：Tool 是异步的（`AsyncGenerator`），可以支持任意长的操作（包括网络请求、文件 I/O）。

---

## 关键要点

1. **Tool interface 是客户端与 Agent 的合约**，包含名称、描述、schema、执行函数
2. **Permission 检查是 tool 执行前的网关**，决定是否允许
3. **并发执行最多 5 个 tool**，但结果必须按调用顺序返回
4. **Sibling abort 是快速失败**，一个错误就停止兄弟 tool
5. **AsyncGenerator 使 tool 支持 streaming**，UI 可见进度，model 可见中间步骤

---

## 深入阅读

- `Tool.ts`：Tool interface 定义
- `tools.ts`：Tool 注册和过滤
- `services/tools/StreamingToolExecutor.ts`：并发执行编排
- `utils/permissions/permissions.ts`：权限检查

下一步：去了解**多 Agent 协调**，看看当 Claude 想要 spawn 一个子 agent 时（通过 `AgentTool`），系统如何支持这个。
