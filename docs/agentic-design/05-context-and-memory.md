# Context 和 Memory：有限的脑容量

## 为什么这很重要？

Claude 的 context window（比如 200K tokens）是**有限的**。一个长时间运行的 agent task：

```
第 1 轮：用户输入（200 tokens）+ 响应（1000 tokens）
第 2 轮：新输入（300 tokens）+ 响应（2000 tokens）
...
第 100 轮：？

总 tokens: 200 + 300 + 400 + ... + 整个历史

在第 50 轮时：messages 数组有 100 条消息，超过 context window 的 80%
```

一旦超限，系统必须做些什么。Claude Code 的答案是：

1. **System prompt 优化**：区分静态和动态部分，利用 prompt cache
2. **主动 compaction**：在超限前压缩历史
3. **长期 memory**：用户的持久化知识库

---

## System Prompt 架构

不是简单的一个大 string，而是**模块化的**：

```typescript
const systemPrompt = [
  // 第 1 部分：静态，会被缓存
  SYSTEM_PROMPT_PREFIX,         // 关于 Claude Code 的基础指导
  TOOL_DESCRIPTIONS,            // 所有 tool 的描述
  SYSTEM_PROMPT_SAFETY,         // 安全指导

  // 缓存边界 ← 这之后的部分会变化，不会被缓存
  "SYSTEM_PROMPT_DYNAMIC_BOUNDARY",

  // 第 2 部分：动态，每个 session 不同
  USER_MEMORY,                  // 用户的个人知识库
  RECENT_CONTEXT,               // 最近的几轮对话（总结）
  CURRENT_TASK,                 // 当前任务的上下文
]
```

**Cache boundary marker** 的妙处：

```
初始化时：                     │首个请求：
cache_key = MD5(prefix)       │所有文本
缓存大小：10 KB               │2000 tokens (40 KB)
                              │
                              │缓存命中 10 KB
                              │处理 30 KB 新文本
                              │总成本：40 KB 请求
                              │
同一 session 中再次使用：      │再次请求：
cache 仍有效                  │新文本 1000 tokens (20 KB)
直接跳过了 10 KB 的重复传输   │缓存 hit（10 KB）
成本：20 KB                   │总成本：20 KB 请求（节省 10 KB）
```

所以，通过把 system prompt 分为静态和动态两部分，我们：
- 减少了重复 token 消耗
- 加快了推理速度（缓存的部分直接使用）
- 提高了吞吐量（多个请求共享缓存）

相关文件：`utils/messages/systemInit.ts`, `context.ts`

---

## Token Budget 生命周期

### 阶段 1：初始化

```typescript
const CONTEXT_WINDOW = 200_000  // 比如 200K tokens
const SYSTEM_PROMPT_TOKENS = 5_000
const RESERVED_BUFFER = 10_000  // 总是留出来，防止意外超限

available = CONTEXT_WINDOW - SYSTEM_PROMPT_TOKENS - RESERVED_BUFFER
          = 185_000 tokens  // 工作预算
```

### 阶段 2：追踪使用

每一轮 API 调用后，系统获得实际使用数据：

```typescript
response = await claude.messages.create({...})

remaining = available - response.usage.input_tokens - response.usage.output_tokens

if (remaining < available * 0.2) {  // 还剩 20% 以下？
  console.warn("Token budget 即将耗尽，触发 compaction")
}

if (remaining < 10_000) {  // 紧急状态？
  console.error("无法继续，context 严重超限")
  return ERROR_OUT_OF_TOKENS
}
```

### 阶段 3：警告和恢复

```
Threshold 1（80% 已用）：显示警告，继续
  ↓
Agent 执行更多 tool  ← 看不到警告，继续工作
  ↓
Threshold 2（90% 已用）：触发 compaction
  ← 在后台自动压缩，不中断 agent 工作
  ↓
Threshold 3（99% 已用）：停止接受新请求
  ← 返回错误，等待用户决定
```

相关文件：`query/tokenBudget.ts`

---

## Compaction：自动历史压缩

当 token 预算吃紧时，系统自动压缩历史。这是一个 4 步流程：

### 第 1 步：分析历史

```typescript
// 扫描所有消息，找出"能压缩的部分"

messages = [
  system_prompt,     // 不能压缩
  message_1,         // 用户说："帮我读个文件" → 不压缩（用户消息）
  message_2,         // Claude 说："我读了，代码如下..." → 能压缩（输出很长）
  message_3,         // 用户说："现在修复 bug" → 不压缩
  message_4,         // Claude 说："修复完，改了 5 处..." → 能压缩
  ...
]

优先级：越早的消息优先级越高（假设最早的对话不再相关）
```

### 第 2 步：优先排序

```
对能压缩的消息排序：
  1. 最早的长消息（4 messages ago）
  2. 次早的长消息（6 messages ago）
  3. ...

实际压缩的可能是：
  messages 1-10（最早的 10 条）被替换为 1 条 summary
```

### 第 3 步：调用 Claude 做总结

```typescript
const summary = await claude.messages.create({
  messages: [
    { role: "user", content: "Summarize this conversation in 200 lines max:\n" + messagesText }
  ],
  system: "You are a memory consolidation assistant..."
})

// 返回：
// "用户让我读了 package.json 和 tsconfig.json，发现项目是 TypeScript monorepo。"
```

### 第 4 步：替换

```typescript
// 替换前：
messages = [msg_1, msg_2, ..., msg_100]  // 100 条消息

// 替换后：
messages = [msg_summary, msg_51, msg_52, ..., msg_100]
//          ^被压缩成1条    ^保留后面的消息（近期的）
```

结果：从 100 条消息减少到 51 条，token 预算恢复。

相关文件：`query.ts` 中的 `compactMessages()` 函数，大约 200 行

---

## Memory 系统：长期知识库

Compaction 是**临时的**（压缩 session 内的历史）。但如果用户有**多个 session**，同一信息会被重复压缩。

解决方案：**持久化 memory**。

### Memory 文件结构

```
~/.claude/memory/
├── MEMORY.md              ← 用户的长期知识库（最多 200 行）
├── projects/
│   └── claude-code.md     ← 项目特定的知识
├── patterns/
│   └── async-patterns.md  ← 学到的模式
└── people/
    └── team.md            ← 关于人的信息
```

**MEMORY.md** 的内容示例：

```markdown
# 我的知识库

## 工作偏好
- 不喜欢在 main 分支上直接提交
- 总是用 TypeScript，避免 JavaScript

## 项目信息
- Claude Code 仓库: ~/projects/claude-code
- 技术栈: TypeScript + React + Ink

## 编码习惯
- 函数 <100 行为最佳
- 避免 deep nesting，最多 3 层
- 总是加 error handling
```

### Auto-Dream：后台内存巩固

不是用户手动维护 MEMORY.md，而是系统**自动**从 session 中提取和更新。

过程（"dream"）：

```
触发条件（三门齐全）：
  1. 时间门：距离上次 dream ≥ 24 小时？
  2. 会话门：距离上次 dream，产生了 ≥ 5 个新 session？
  3. Lock 门：能获得 consolidation lock（防并发）？

都满足 → 执行 dream：

[1] ORIENT
    读取用户的 MEMORY.md
    扫描最近 N 个 session 的 transcripts

[2] GATHER
    提取新的学习点：
      - "用户在这个 session 中学到了异步 Rust"
      - "发现项目从 4.1 升级到了 4.2"
      - "优化了 CI/CD，速度提升 30%"

[3] CONSOLIDATE
    合并到 MEMORY.md：
      - 如果已经有同类笔记，合并
      - 如果是新的，添加
      - 保持在 200 行以内（做优先级选择，删除过时的）

[4] PRUNE & INDEX
    - 删除 24 小时内不用的 session transcript
    - 更新搜索索引（便于后续 memory 查询）
```

相关文件：
- `services/autoDream/autoDream.ts`：主逻辑（~11 KB）
- `services/autoDream/consolidationPrompt.ts`：告诉 Claude 怎么总结
- `memdir/memdir.ts`：Memory 目录管理

### 为什么自动化很关键？

```
手动维护：
  用户要记得更新 MEMORY.md
  → 大部分人会忘记
  → Memory 逐渐变陈旧

自动化：
  后台每 24 小时自动 consolidate
  → 用户无感
  → Memory 总是最新的
```

---

## Context 使用的优化层级

当 context 变紧时，系统依次采取行动：

```
Tier 1（还有 70% token）
  ✓ 正常工作
  ✗ 触发自动 compaction

Tier 2（还有 20% token）
  ✓ 继续工作（使用压缩后的历史）
  ✗ 不接受新的大型工具输出（如读 10 MB 文件）

Tier 3（还有 10% token）
  ✓ 只允许低 token 成本的操作
  ✗ 禁止高 token 成本的操作

Tier 4（还有 < 5% token）
  ✗ 停止工作，返回错误
  → 用户可以：
     a) 启动新 session（重置 token 预算）
     b) 手动编辑 MEMORY.md，删除过时信息
```

---

## Design Decision 专栏

### 为什么区分静态和动态 system prompt?

不区分（一大块）：
```
发第 1 个请求：发 5000 tokens 的 system prompt + 输入
发第 2 个请求：又发 5000 tokens 的 system prompt + 输入
...
浪费了大量 token 在重复发送
```

区分（cache boundary）：
```
发第 1 个请求：发 5000 tokens（静态）+ 1000 tokens（动态）
              API 缓存这 5000 tokens
发第 2 个请求：直接用缓存的 5000 tokens + 1000 tokens（新的动态）
              节省了 5000 tokens！
```

如果 session 很长（100 次请求），节省 = 5000 * 99 = 495K tokens！

### 为什么 Compaction 触发阈值是 20%，而不是 1%？

等到 1% 时才 compact：
```
已用 99%，只有 1% 剩余（~2000 tokens）
这时启动 compaction，自己就需要消耗大量 tokens（调用 Claude 做总结）
可能没有足够 tokens 来完成 compaction
→ 失败，系统崩溃
```

提前到 20% 时 compact：
```
已用 80%，还有 20% 剩余（~40K tokens）
这时启动 compaction，有充足 tokens 来做总结
完成后，token 预算恢复到 50%
→ 继续工作，很多时间都有富足的 token 可用
```

权衡：早点 compact 意味着多付出一些 token（压缩的成本），但换来系统稳定性。

### 为什么用后台 dream 而不是 inline compaction?

Inline（同步）：
```
在 query loop 中调用 compaction
  ↓
等待 compaction 完成（5-10 秒）
  ↓
用户看到卡顿
```

后台 dream：
```
query loop 继续运行
  ↓
在空闲时或定时触发 dream（24 小时一次）
  ↓
用户感觉不到延迟
```

代价：增加系统复杂性（需要处理并发）、需要文件锁（防同时 dream）。但值得，因为用户体验好得多。

---

## 常见误解

**误解 1**："Context window 满了就完蛋？"

实际：有多层缓冲和恢复机制。系统会逐步：
1. 显示警告
2. 触发 compaction
3. 拒绝新操作
4. 要求用户决定

完全"卡住"很少发生。

**误解 2**："Compaction 会丢失所有细节？"

实际：Compaction 由 Claude 做，所以**关键信息被保留**。丢失的是：
- 冗余的解释
- 尝试失败的细节（对后续不相关）
- 已解决的问题的讨论过程

**误解 3**："MEMORY.md 会无限增长？"

实际：维持在 ~200 行（可配置），通过 dream 的 prune 阶段定期清理过时信息。

---

## 关键要点

1. **System prompt 分两部分**：静态（可缓存）+ 动态（每次变化）
2. **Token budget 主动追踪**：到达 80% 时触发 compaction
3. **Compaction 自动压缩历史**：用 Claude 总结 N 条消息为 1 条
4. **Memory 系统持久化知识**：MEMORY.md 最多 200 行，用户的长期知识库
5. **Auto-dream 后台巩固**：每 24 小时自动更新 MEMORY.md，无需用户介入
6. **优化层级**：70% → 20% → 10% → 5% → 停止，每个阈值有对应策略

---

## 深入阅读

- `query/tokenBudget.ts`：Token 追踪逻辑
- `services/autoDream/autoDream.ts`：Dream 实现
- `memdir/memdir.ts`：Memory 目录管理
- `utils/messages/systemInit.ts`：System prompt 组装

下一步：了解**Feature Gating 系统**，看看 Claude Code 怎么区分内部功能和公开功能。
