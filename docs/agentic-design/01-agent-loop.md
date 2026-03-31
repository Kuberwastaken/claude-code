# 核心 Query Loop：Agent 的心脏

## 为什么需要 Loop?

想象一个简单的"一问一答"的 AI：用户问 → API 回复 → 完成。但 agent 不一样：

- 用户问："帮我修复 bug"
- Agent 读取文件 → 分析问题 → 尝试修复 → 运行测试 → 测试失败 → 再次修改 → 再次测试 → 成功

这个过程需要**多轮往返**，每一轮中 Agent 决定"下一步应该执行哪个 tool"。这就是 **Query Loop**。

---

## 核心概念：状态机

Query loop 本质上是一个**状态机**，在以下几个状态间循环：

```
START
  ↓
QUERY (提交 prompt 给 Claude API)
  ↓
TOOL_USE (API 返回 tool_use block，包含 tool 名 + 输入)
  ↓
执行 Tool (BashTool, FileEditTool, 等等)
  ↓
RESULT (Tool 返回结果)
  ↓
提交结果给 Claude (作为 user 消息)
  ↓
QUERY (继续循环)
  ↓
END (API 返回 stop_reason="end_turn" 或其他终止条件)
```

---

## 代码位置

关键文件：`query.ts` (~1700 行)

这个文件包含：
- Loop 的完整实现 (使用 `AsyncGenerator`)
- 状态跟踪
- Tool 调用的编排
- Streaming 支持
- Compaction 触发
- 错误处理和恢复

你应该从 `query.ts` 的函数签名开始：

```typescript
export function* query(...): AsyncGenerator<QueryMessage | QueryEvent, ...>
```

它返回一个 `AsyncGenerator`，每次 `yield` 都是一个"事件"（API 消息、tool 调用、错误等）。

---

## Loop 的生命周期

### 阶段 1: 初始化

当你调用 `query()` 时：
- `messages` 数组初始化（包含 system prompt + 历史消息）
- `token_budget` 初始化（计算剩余 token）
- Compaction 检查（如果历史太长，自动压缩）
- Abort controller 初始化（支持中途停止）

### 阶段 2: 发送 Prompt

```typescript
// 构建消息数组
let messages = [system_prompt, ...history];

// 调用 Claude API（使用缓存）
let response = await claude.messages.create({
  messages,
  system: system_prompt,
  max_tokens: remaining_budget,
  stream: true,  // 重要：流式输出
  ...config
});
```

Streaming 很关键——API 会逐步返回：
1. First chunk: 可能包含 `message.start`
2. Content blocks: `text_delta`, `tool_use` block 开始
3. Stop reason: 表示本轮结束

### 阶段 3: 处理 Streaming

当 API 返回 `tool_use` block 时，loop 收集完整的参数，然后：

```
接收 tool_use block
  ↓
验证 tool 名称是否存在
  ↓
检查权限 (useCanUseTool hook)
  ↓
执行 tool (StreamingToolExecutor)
  ↓
Yield tool 结果给消费者
  ↓
添加到 messages 作为 user 消息
```

### 阶段 4: 工具执行

Tool 执行由 `services/tools/StreamingToolExecutor.ts` 处理：
- **并发**：最多 5 个 tool 同时运行
- **顺序输出**：结果按调用顺序返回（即使执行顺序不同）
- **Sibling abort**：一个失败会 cancel 兄弟 tool

### 阶段 5: 检查终止条件

每一轮后，loop 检查是否应该继续：

```
stop_reason == "end_turn"?      → 正常结束
stop_reason == "max_tokens"?    → Token 超限，触发 compaction
consecutive_errors >= 3?        → 连续错误，放弃
max_turns_reached?              → 超过最大轮数
abort_signal?                   → 用户中断
```

---

## Token Budget：Context Window 的有限性

Context window 是有限的（比如 200K tokens）。这里的逻辑：

```
available_tokens = context_window_size - system_prompt_tokens - reserved_buffer

每一轮后：
  usage = response.usage (API 返回的实际使用)
  remaining = available_tokens - usage.input_tokens - usage.output_tokens

  if remaining < threshold:
      触发 compaction (压缩历史)

  if remaining < min_required:
      拒绝继续，返回错误
```

**Threshold** 通常设得比较激进，比如还剩 20% 时就开始压缩，以避免在真的耗尽时措手不及。

相关文件：`query/tokenBudget.ts`

---

## Compaction：自动历史压缩

当 token 预算吃紧时，loop 会自动压缩历史：

1. **Analyzer phase**: 扫描历史消息，找出"长输出消息"（比如代码块）
2. **Prioritize**: 对话越早，压缩优先级越高
3. **Compress**: 使用 Claude API 总结这段历史，生成 200 行以内的 summary
4. **Replace**: 把原始的 20 条消息替换为 1 条 summary 消息

这保证了 loop 可以持续运行，即使初始历史很长。

---

## Streaming 与 Tool Call 的交织

这是一个复杂的地方。API 可能返回：

```
message {
  content: [
    { type: "text", text: "让我先读文件..." },
    { type: "tool_use", id: "...", name: "FileReadTool", ... },
    { type: "text", text: "现在我看到问题了..." },
    { type: "tool_use", id: "...", name: "FileEditTool", ... },
  ]
}
```

所以一条消息中可能**混合了文本和 tool call**。Loop 需要：
- 收集每个 tool_use block 的完整输入参数（可能分段到达）
- 在 tool_use 结束时立即执行（不等待消息全部返回）
- 继续接收后续的文本或 tool call
- 当消息完全接收后，将所有 tool 结果作为一条 user 消息发回

这使得 **interleaved thinking** 成为可能：模型可以一边思考一边调用工具。

---

## Design Decision 专栏

### 为什么用 `AsyncGenerator` 而不是 callback?

不好的设计（callback）：
```typescript
query(messages, {
  onMessage: (msg) => {...},
  onToolCall: (tool, input) => {...},
  onError: (err) => {...},
  onProgress: (progress) => {...},
})
```

问题：
- 4 种不同的 callback，难以理解执行顺序
- 消费者需要管理状态机来追踪"当前在哪个阶段"
- 测试困难

**更好的设计**（Generator）：
```typescript
for await (const event of query(messages, config)) {
  if (event.type === 'message') {...}
  else if (event.type === 'tool_use') {...}
  else if (event.type === 'error') {...}
}
```

优点：
- 单一的 `for await...of` 循环，清晰的顺序
- 每个 event 都是一个联合类型，编译器可以帮你检查
- 可以在 loop 中添加复杂的条件逻辑（abort, timeout 等）
- 易于测试（可以模拟一个生成事件的 generator）

Generator 让"一系列事件"变得一级公民。

### 为什么需要 Compaction?

不做 compaction：
- 100 轮 agent 工作后，messages 数组有几千条消息
- 提交给 API 时，prompt cache hit 率下降（因为前缀不再是"静态部分"）
- Token 浪费在重复的对话历史上

做 compaction：
- 定期将"已经达成共识的历史部分"总结成一条消息
- 保持 prompt cache 的"静态前缀"有效
- 节省 token，延长 agent 能工作的轮数

---

## 常见误解

**误解 1**："Query loop 就是不断问 Claude？"

实际：Loop 实现的是一个**两层状态机**：
- 外层：Claude API 的请求/响应循环
- 内层：Tool 的执行和结果收集

tool 执行完全不涉及 Claude API 的额外调用（除非 tool 本身调用 API）。

**误解 2**："Compaction 会丢失信息？"

实际：Compaction 使用 Claude 自己来总结历史，所以关键信息被保留。丢失的只是"冗余的解释"或"中间尝试"，这些对后续工作没用。

**误解 3**："Token budget 追踪是精确的？"

实际：API 返回的 `usage` 是经过舍入的（某些模型），所以我们的估计可能有 ±5% 的误差。这就是为什么我们用激进的阈值（20% 时就开始压缩）而不是等到 100% 才反应。

---

## 关键要点

1. **Query loop 是异步生成器**，yield 事件流而不是回调
2. **循环的关键状态转移**：QUERY → TOOL_USE → RESULT → QUERY
3. **Token budget 必须主动管理**，compaction 是自动压缩的关键
4. **Streaming 支持 interleaved thinking**，一条消息中可以混合文本和 tool call
5. **Compaction 会定期触发**，保持 history 可控，并维护 prompt cache 的有效性

---

## 深入阅读

- `query.ts`：完整 loop 实现（必读）
- `query/tokenBudget.ts`：Token 追踪逻辑
- `services/tools/StreamingToolExecutor.ts`：Tool 执行编排
- `query/stopHooks.ts`：终止条件检查

下一步：去了解 **Tool 系统**是如何设计的，使得这个 loop 可以灵活地执行任意 tool。
