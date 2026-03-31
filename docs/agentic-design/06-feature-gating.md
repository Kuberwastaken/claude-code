# Feature Gating：内部功能和公开功能的隔离

## 为什么需要 Feature Gating?

Claude Code 是一个复杂系统，有很多功能：

- 一些已发布、稳定、所有用户可用
- 一些还在测试、只给内部用户（"ant" 用户）
- 一些只给特定高级用户（Max/Pro)
- 一些通过实验 A/B 测试

如果不做隔离，代码会变得一团糟：

```typescript
// 不好的做法
if (user.isInternal) {
  // KAIROS 模式逻辑
} else if (user.isMax) {
  // 高级功能
} else if (featureFlags.has('experimental_search')) {
  // A/B 测试
} else {
  // 默认行为
}
// ... 这样嵌套太深，难以维护
```

Claude Code 的解决方案：**编译期和运行期双层 gating**。

---

## 编译期 Gating：Bun 的 `feature()` API

Bun（JavaScript 运行时）提供了一个编译期特性检测 API：

```typescript
if (feature("KAIROS")) {
  // 这段代码只在 KAIROS 构建中出现
  // 其他构建完全不包含这代码
} else {
  // 默认行为
}
```

### 工作原理

```
源代码：
  if (feature("KAIROS")) {
    launchKAIROS()
  } else {
    launchDefault()
  }
      ↓ Bun 编译（--feature KAIROS）
  if (true) {
    launchKAIROS()  // 只保留这段
  } else {
    // 被 dead code elimination 删除
  }
      ↓ 输出的二进制
  launchKAIROS()  // 这是最终的可执行代码
```

### 优势

1. **二进制体积小**：内部功能完全不包含在公开二进制中
2. **安全**：用户不能"解锁"内部功能（不存在于二进制）
3. **性能**：不需要运行时检查这些条件

相关代码散布在整个 codebase 中，比如 `entrypoints/cli.tsx`, `bootstrap/state.ts`。

---

## Compile-Time Feature 列表

Claude Code 在构建时支持这些 feature flag：

| Flag | 启用时 | 作用 |
|------|--------|------|
| `KAIROS` | 启动 KAIROS mode（always-on Claude） | 全天候 assistant |
| `PROACTIVE` | 同上（别名） | 同上 |
| `BRIDGE_MODE` | 启用 claude.ai 远程控制 | 在 Web UI 中控制 CLI |
| `VOICE_MODE` | 启用语音输入 | 用话筒说指令 |
| `DAEMON` | 启用 daemon 模式 | 后台运行的 Claude |
| `COORDINATOR_MODE` | 启用 coordinator 多 agent | 复杂任务分解 |
| `BUDDY` | 启用 Tamagotchi 伴侣系统 | 养一个宠物 AI |
| `WORKFLOW_SCRIPTS` | 启用工作流脚本 | 自动化重复任务 |
| `NATIVE_CLIENT_ATTESTATION` | 启用本地证明 | 设备信任 |
| `TRANSCRIPT_CLASSIFIER` | 启用 AFK 自动模式 | 离开电脑时自动工作 |
| `HISTORY_SNIP` | 启用历史压缩优化 | Context 更高效 |
| `EXPERIMENTAL_SKILL_SEARCH` | 启用技能搜索（实验） | 发现新 skill 命令 |
| `ABLATION_BASELINE` | 科学研究 baseline | 论文用 |

默认只编译：`KAIROS=0, BRIDGE_MODE=0, ...`（全是 0，最小二进制）

内部构建时：编译脚本设置 `--feature KAIROS=1 --feature BRIDGE_MODE=1 ...`

相关文件：构建脚本（如 `scripts/build.sh`，如果存在），或 `bunfig.toml`

---

## 运行期 Gating：GrowthBook Feature Flags

编译期 feature 决定"可能性"，运行期 flags 决定"激活"。

系统启动时，从 GrowthBook（特性管理平台）获取动态配置：

```typescript
// 启动时
const flags = await growthbook.getFeatures({
  userId: currentUser.id,
  organization: currentUser.org
})

// 现在可以做运行期决策
if (flags.has('tengu_scratch')) {
  // 启用共享 scratchpad（coordinator 模式）
  enableScratchpad()
}

if (flags.has('tengu_amber_flint')) {
  // 启用 in-process teammate swarms
  enableTeammates()
}

if (flags.has('tengu_penguins_off')) {
  // 禁用某个实验
  disableExperiment()
}
```

### Flag 命名约定

Runtime flags 以 `tengu_` 开头（"tengu" 是 Claude Code 的内部代号）：

```
tengu_scratch              # 共享 scratchpad（为 coordinator）
tengu_amber_flint          # In-process teammate
tengu_onyx_plover          # 某个实验
tengu_penguins_off         # 禁用什么功能
...
```

这样容易区分：
- `KAIROS`（编译期）vs `tengu_kairos_v2`（运行期）

相关文件：`bootstrap/state.ts`（初始化），`services/analytics/`（GrowthBook 集成）

---

## 两层系统的协作

### 场景 1：Compile-Time Only（最常见）

```
新功能 KAIROS（always-on Claude）
  ↓ 编译
  ├─ 构建 ant（内部）：--feature KAIROS=1
  │   → KAIROS 代码被编译进去
  │
  └─ 构建 public（公开）：--feature KAIROS=0
      → KAIROS 代码被 dead-code-elimination 删除
      → 二进制不含任何痕迹
```

### 场景 2：Compile-Time + Runtime（用于 A/B 测试）

```
新的 memory 系统（已编译进去）
  ↓ 运行时
  ├─ 用户在 tengu_new_memory=true 组（50%）
  │   → 使用新系统
  │
  └─ 用户在 tengu_new_memory=false 组（50%）
      → 使用旧系统
      → 收集对比数据
```

### 场景 3：运行期特定用户

```
高级功能（编译期编译进去）
  ↓ 运行时
  ├─ Max 订阅用户 → 启用计算机使用
  ├─ Pro 订阅用户 → 启用高级 memory
  └─ 免费用户 → 基础功能
```

相关文件：`services/policyLimits/`（根据订阅级别应用 policy）

---

## 对 System Prompt 的影响

Feature gating 会改变 system prompt 的内容：

```typescript
const systemPrompt = [
  BASE_PROMPT,

  // 只有在编译时启用 BRIDGE_MODE 时才包含
  feature("BRIDGE_MODE") ? BRIDGE_MODE_INSTRUCTIONS : "",

  // 运行时检查：如果用户有 tengu_coordinator flag
  flags.get("tengu_coordinator") ? COORDINATOR_INSTRUCTIONS : "",

  // 运行时 API beta 特性
  BETAS_NEGOTIATED_WITH_API,  // 如 "interleaved_thinking"
]
```

这也影响 **prompt cache**：

```
缓存键 = MD5(system_prompt_prefix)

如果 flag 变化，cache key 变化，旧 cache 失效
```

所以，高频变化的 runtime flag 应该放在 cache boundary 之后（动态部分），而不是之前（静态部分）。

---

## Betas 协商

Claude API 定期发布新特性（beta），Claude Code 需要：

1. 请求启用这些 beta
2. 如果启用，在 system prompt 中告诉 Claude（这个 API 支持什么新功能）

例子：

```typescript
// constants/betas.ts
const BETAS_REQUESTED = [
  "interleaved-thinking",        // 支持在回复中穿插思考
  "structured-outputs",           // 支持返回结构化 JSON
  "context-1m",                   // 100 万 token context
  "web-search",                   // 网络搜索能力
]

// API 返回确认
const BETAS_ENABLED = [
  "interleaved-thinking",         // ✓ 已启用
  // "structured-outputs",        // ✗ 暂无权限
  "context-1m",                   // ✓ 已启用
  "web-search",                   // ✓ 已启用
]

// System prompt 中加入这些信息
You have access to the following beta features:
  - Interleaved thinking: You can output <thinking> blocks
  - Context 1M: You can use up to 1,000,000 tokens
  - Web search: You can call the WebSearchTool
```

相关文件：`constants/betas.ts`

---

## Design Decision 专栏

### 为什么同时用编译期和运行期?

只用编译期：
```
缺点：无法做快速的 A/B 测试（需要重新编译）
      无法根据用户身份动态启用功能
```

只用运行期：
```
缺点：内部功能可能被反编译/逆向工程
      内部代码暴露在公开二进制中
      启动时需要网络请求拿 flags（慢）
```

两者结合：
```
编译期决定"可能性"（物理隔离内部代码）
运行期决定"激活"（灵活的 A/B 测试）
最安全、最灵活
```

### 为什么 Bun 的 feature() 而不是其他工具?

Bun 的 feature() 是编译期指令，会进行 dead code elimination：

```
其他工具（如 rollup 的条件编译）：
  需要额外的 webpack 插件
  编译配置复杂

Bun:
  原生支持
  编译快（Bun 本身就快）
  输出体积最小
```

---

## 常见误解

**误解 1**："Runtime flag 可以改变编译期行为？"

实际：不能。Runtime flag 只能在编译期已包含的代码中做选择。如果代码在编译时被 dead-code-elimination 删除了，运行时再想启用也没办法。

```typescript
// 如果编译时 --feature KAIROS=0，这段代码被删除
if (feature("KAIROS")) {
  launchKAIROS()
}

// 运行时即使 growthbook.flags.get("enable_kairos") == true，也没用
// 代码不存在于二进制中
```

**误解 2**："Feature flag 对性能有开销？"

实际：编译期 feature flag 没有开销（代码级别选择）。运行期 flag 有极小开销（map lookup），可忽略不计。

**误解 3**："所有用户都能看到内部代码？"

实际：不能。公开二进制中编译时被删除的代码，用户看不到。只有 ant（内部）用户的二进制才包含这些代码。

---

## 关键要点

1. **编译期 feature**：Bun 的 `feature()` API，dead code elimination，对二进制体积和安全性的保障
2. **运行期 flags**：GrowthBook，动态启用特性，支持 A/B 测试
3. **两层协作**：编译决定可能性，运行时决定激活
4. **System prompt 影响**：不同 flag 组合 → 不同 system prompt → 不同 cache key
5. **API Beta**：协商启用新的 Claude API 特性，在 system prompt 中告诉 Claude

---

## 深入阅读

- `bootstrap/state.ts`：运行时 flag 初始化（56 KB）
- `constants/betas.ts`：Beta feature 列表
- `constants/system.ts` 和 `constants/systemPromptSections.ts`：System prompt 组装
- `entrypoints/cli.tsx`：编译期条件编译示例

---

## 后记

这 6 篇文档覆盖了 Claude Code 最核心的 agentic 设计决策。还有很多其他话题（Bridge 模式、Voice 输入、Plugin 系统等），但这些是最重要的基础。

如果你想进一步了解，建议：
1. 读完这 6 篇
2. 打开 `query.ts` 和 `Tool.ts`，对照源代码
3. 运行 Claude Code，用 `--verbose` flag 看内部日志
4. 探索 `tools/AgentTool/` 的源代码

祝学习愉快！
