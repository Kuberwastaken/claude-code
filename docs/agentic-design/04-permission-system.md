# Permission 系统：Agent 的沙箱

## 为什么需要权限管控?

想象 Claude 可以执行任意 bash 命令。用户说"帮我看看网站"，Claude 可能：
- 合法地执行 `curl https://example.com`
- 也可能执行 `rm -rf /` ← 灾难

**权限系统的目标**：
1. 阻止意外的危险操作
2. 提示用户"你确定吗？"
3. 在保留便利的同时维护安全

Claude Code 的 permission 系统是一个**多层防线**。

---

## 核心概念：Permission Mode

系统支持多种权限模式，用户可以选择：

| Mode | 特点 | 场景 |
|------|------|------|
| `default` | 每次都问用户 | 不信任 agent，想完全控制 |
| `auto` | ML 分类器自动审批 | 信任 agent，想要流畅体验 |
| `acceptEdits` | 自动接受文件编辑（但仍然问危险操作） | 开发工作流（改代码 OK，执行脚本要问） |
| `bypass` | 完全自动，不问（谨慎使用）| 内部测试、自动化流程 |
| `dontAsk` (aka `yolo`) | 拒绝所有非低风险操作 | 沙箱环境，最安全 |
| `plan` | 特殊模式：进入计划编辑阶段 | 用户审核计划后再执行 |

用户在启动时选择（环境变量或配置文件）：
```bash
# 每次都问
claude-code --permission-mode default

# 自动审批
claude-code --permission-mode auto

# 最安全
claude-code --permission-mode dontAsk
```

相关文件：`utils/permissions/PermissionMode.ts`

---

## Permission 决策 Pipeline

当 agent 试图执行一个 tool 时，系统会问："我应该允许吗？"

```
Agent 调用: BashTool { command: "npm test" }
  │
  ├─ [1] Mode 检查
  │      if mode == "bypass" → ✓ 允许，返回
  │      if mode == "dontAsk" → ✗ 拒绝 Bash，返回
  │
  ├─ [2] 规则匹配
  │      检查用户配置的规则列表
  │      如: "Bash(git *)" → 允许 git 命令
  │         "Bash(npm *)" → 允许 npm 命令
  │         "Bash(*)" → 允许任意 bash （太宽）
  │
  │      我们的命令 "npm test" 匹配 "npm *" → ✓
  │
  ├─ [3] 风险分类
  │      rule match → LOW 风险
  │      如果没匹配到规则，分类器决定风险等级
  │
  ├─ [4] YOLO 分类器
  │      对于 MEDIUM 风险的操作：
  │      - 如果被模型认为"明显安全"，自动批准
  │      - 否则需要用户确认
  │
  └─ [5] 最后手段：提示用户
         用户选择: ✓ 允许 / ✗ 拒绝 / ⚠️ 改规则
```

相关文件：`utils/permissions/permissions.ts` (52 KB，核心决策逻辑)

---

## 规则系统

用户可以配置规则，告诉 agent 什么是自动允许的。规则语法：

```
Bash(git *)        # 允许任何 git 命令
Bash(npm install)  # 只允许 npm install，不允许 npm test
FileEdit(src/*.ts) # 允许编辑 src/ 下的 .ts 文件
FileRead(/*)       # 允许读任何文件
```

规则支持 **glob 模式**（`*` 通配符）。

### 规则分层

规则可以来自多个地方（优先级从高到低）：

1. **CLI 参数**：`--approve-file "FileEdit(src/*.ts)"`
2. **项目配置**：`.claude/settings.json` 中的 `approvals`
3. **用户设置**：`~/.claude/settings.json`
4. **系统默认**：Claude Code 内置的基础规则

下层规则会被上层规则覆盖。

相关文件：`utils/permissions/permissionRuleParser.ts`, `utils/permissions/permissionsLoader.ts`

---

## YOLO 分类器：ML 自动审批

对于**规则未覆盖的操作**，system 用 YOLO 分类器（"You Only Live Once"）决定：

```
输入:
  tool_name: "BashTool"
  command: "npm test"
  context: 当前任务是"修复 bug"

分类器说: "这个操作很像是开发者会做的正常事，批准"

结论: 自动允许，不问用户
```

分类器基于 ML 训练，考虑：
- Tool 类型（BashTool 比 FileReadTool 高风险）
- 具体命令（`npm test` 比 `rm -rf` 安全）
- 当前上下文（"修复 bug" vs "浏览网页"）

相关文件：`utils/permissions/yoloClassifier.ts` (52 KB)

### 分类器的风险等级

```
LOW
  → 自动允许（不问用户）
  → 例：FileReadTool, GlobTool, WebFetchTool

MEDIUM
  → 取决于 mode（auto mode 用分类器，default mode 问用户）
  → 例：FileEditTool, BashTool (git 命令)

HIGH
  → 一般不自动允许（需要用户确认）
  → 例：BashTool (rm -rf), FileWriteTool (覆盖重要文件)
```

---

## Denial Tracking：Circuit Breaker

如果用户连续拒绝 agent 多次，system 会自动**降级权限模式**：

```
用户拒绝:
  1. "BashTool"  ← Denial #1
  2. "FileEditTool" ← Denial #2
  3. "BashTool" ← Denial #3

连续 3 次拒绝触发!
  ↓
系统切换到 "default" mode，后续所有操作都要问用户

这样防止：
- Agent 持续尝试被拒的操作
- 用户一直看到提示烦不胜烦
```

同时，系统还追踪**总拒绝数**：

```
同一 session 中总共拒绝了 20 次
  ↓
系统记录日志："这个 session 决策质量差，建议用户审视任务"
```

相关文件：`utils/permissions/denialTracking.ts`

---

## 路径安全：防止路径遍历攻击

即使规则写得再好，攻击者也可能用**特殊编码**绕过：

```
规则: FileEdit(src/*.ts)  ← 只允许 src/ 目录

攻击尝试:
  path: "src/../../../etc/passwd"  ← 试图逃逸
  path: "src%2f..%2f..%2fetc%2fpasswd" ← URL 编码
  path: "src/\u202e../../../etc/passwd" ← Unicode 隐藏字符
```

系统有多层防御：

1. **Path Normalization**：
   ```
   "src/../../../etc/passwd"
     → 规范化
   → "/etc/passwd"
     → 检查是否在 src/ 内? 否 → 拒绝
   ```

2. **URL Decoding**：
   ```
   "src%2f..%2f..%2fetc"
     → 解码
   → "src/../../etc"
     → 规范化 → 拒绝
   ```

3. **Unicode Normalization**：
   ```
   Unicode 隐藏字符也会被清理
   ```

相关文件：`utils/permissions/pathValidation.ts` (62 KB)

---

## Protected Files：黑名单

某些文件**永远不会被自动编辑**：

```
.gitconfig          ← Git 配置，修改可能破坏工作流
.bashrc, .zshrc     ← Shell 配置，修改可能卡住终端
.mcp.json           ← MCP 配置，影响工具可用性
.claude.json        ← Claude Code 自己的配置
```

这些文件即使通过了权限检查，也会再次提示用户确认。

---

## Design Decision 专栏

### 为什么需要 ML 分类器？

如果只用规则：
```
允许所有 Bash 命令?
  → 太宽，不安全（用户可能 rm -rf）

只允许 git/npm 命令?
  → 太窄，不便利（sudo docker pull 被拒）
```

ML 分类器的优势：
```
可以说"这个命令看起来是在开发工作流的一部分，
而不是破坏性操作，所以允许"

如果出错（classify 有假阴性），系统还有：
  - Denial tracking（连续拒绝降级）
  - Protected files（黑名单）
  - Circuit breaker
```

所以 ML 不是唯一防线，而是**多层防御中的一层**。

### 为什么规则支持 Glob 而不是正则表达式？

Glob：`src/*.ts` → 简单直观
正则：`^src/[^/]+\.ts$` → 复杂，容易写错

目标用户是**工程师，但不是安全专家**。Glob 的错误率更低。

---

## 常见误解

**误解 1**："Permission mode 是全局的，改一次就生效?"

实际：可以在 CLI 参数中针对单个命令覆盖：
```bash
claude-code --permission-mode auto /commit  # 这个 /commit 用 auto mode
claude-code --permission-mode default /config # 这个 /config 用 default mode
```

**误解 2**："YOLO 分类器是完美的？"

实际：分类器是 **best-effort**，有假阳性和假阴性。假阴性（该拒的通过了）会被 denial tracking 捕捉到。

**误解 3**："Protected files 名单是硬编码的？"

实际：可以通过配置扩展：
```json
{
  "protectedFiles": [".gitconfig", ".bashrc", ".my-precious-file"]
}
```

---

## 关键要点

1. **Permission mode**：`default`（每次问）, `auto`（ML 自动）, `acceptEdits`, `bypass`, `dontAsk`
2. **决策 pipeline**：Mode → Rules → Risk Classification → YOLO → User Prompt
3. **规则系统**：Glob 模式，分层来源（CLI > project > user > default）
4. **YOLO 分类器**：ML 自动批准 MEDIUM 风险操作，不需要用户每次确认
5. **Denial tracking**：连续 3 次拒绝自动降级权限，防止烦人的重复提示
6. **路径安全**：多层防御（normalization, URL decode, Unicode clean）
7. **Protected files**：黑名单，永远要再次确认

---

## 深入阅读

- `utils/permissions/permissions.ts`：完整决策逻辑
- `utils/permissions/yoloClassifier.ts`：分类器实现
- `utils/permissions/pathValidation.ts`：路径安全
- `utils/permissions/denialTracking.ts`：Denial tracking
- `utils/permissions/PermissionMode.ts`：Mode 定义

下一步：了解**Context 和 Memory 系统**，看看当 context window 快满时会发生什么。
