# Broker 代理功能技术方案（v1.3.2）

## 概述

Creavor Broker 是一个本地 HTTP 反向代理，拦截 AI 编码工具（Claude Code、OpenCode、Codex、Gemini CLI、Qwen Code）的 API 请求，实现：

- **请求审计**：记录所有请求/响应到 SQLite
- **规则拦截**：基于正则或 LLM 分析检测敏感信息并阻断
- **会话追踪**：通过 session ID 关联请求

### 核心架构

```
┌──────────┐   HTTP 明文    ┌──────────┐   HTTPS 加密   ┌──────────────┐
│ AI 工具   │ ─────────────► │  Broker  │ ──────────────► │ 真实上游 API  │
│          │  127.0.0.1:8765 │  本机    │                │              │
└──────────┘                └──────────┘                └──────────────┘

为什么能看请求内容:
  AI 工具的 baseURL 从 https://api.xxx.com 改为 http://127.0.0.1:8765/v1/<provider>/
  → 第一段是 HTTP 明文（Broker 可读）
  → 第二段是正常 HTTPS（对外加密）
  → Broker 本质上是一个本地 TLS 终结点
```

### 路由规则

Broker 按路径前缀识别 provider：

| 路径前缀 | Provider | 目标上游 |
|---------|----------|---------|
| `/v1/anthropic/*` | Anthropic | Claude API / 兼容代理 |
| `/v1/openai/*` | OpenAI 兼容 | OpenAI / 智谱 / 其他兼容 API |
| `/v1/gemini/*` | Gemini | Google AI API |

### 统一 Header 最简协议（P0）

为避免各工具采用不同的 Header 语义，Broker 对所有 runtime 统一约定以下 **最小 Header 集合**：

| Header | 必选性 | 作用 |
|--------|--------|------|
| `X-Creavor-Runtime` | 必选 | 标识请求来自哪个 runtime，如 `claude-code`、`opencode`、`codex` |
| `X-Creavor-Upstream` | 强烈建议 | 标识 Broker 里的统一上游 ID，如 `zhipu-anthropic`、`anthropic-direct`、`zhipuai-coding-plan`、`zai-openai` |
| `X-Creavor-Session-Id` | 推荐（可缺省） | 标识本地会话或运行实例，用于把 hook 事件流和 API 请求流关联起来；若某 runtime 暂不支持动态注入，可缺省 |

#### Header 语义约定

#### `X-Creavor-Session-Id` 的作用与来源

`X-Creavor-Session-Id` 不是为了路由本身，而是为了把两条原本分离的数据流稳定关联起来：

1. **事件流**：`SessionStart`、`UserPromptSubmit`、`PreToolUse`、`PostToolUse`、`SessionEnd`
2. **请求流**：真正发给 LLM 的 HTTP API 请求

有了 `session_id`，Broker/SQLite 才能按“会话”查看：
- 用户在这一轮会话里说了什么
- 调用了哪些工具
- 最终发送给模型的请求是什么
- 哪些请求被 block

对支持官方会话 ID 的 runtime（如 Claude Code），**优先使用 runtime 官方提供的 session ID**。对暂不支持动态注入的 runtime，`X-Creavor-Session-Id` 不作为 P0 硬依赖。

- `X-Creavor-Runtime`：统一使用稳定枚举值，禁止出现 `claude` / `claude-code` 混用。
- `X-Creavor-Upstream`：表示 **Broker 侧统一上游注册表中的 ID**，不直接等价于协议名，也不要求与工具内部 upstream ID 完全同名。
- `X-Creavor-Session-Id`：主要用于把“会话事件流”（如 `SessionStart`、`UserPromptSubmit`、`PreToolUse`）和“API 请求流”关联起来。优先使用 runtime 官方会话 ID；若某工具暂不支持动态注入，则该 Header 可缺省，Broker 需将请求标记为 `session_unlinked`，或使用时间窗 + runtime + upstream 做 fallback 关联。

> 说明：`X-Creavor-Protocol` 不作为 P0 必填 Header。协议可由 Broker 根据 `X-Creavor-Upstream` 或请求路径（如 `/v1/anthropic/*`、`/v1/openai/*`、`/v1/gemini/*`）自行推导。若后续需要做一致性校验，可把 `X-Creavor-Protocol` 作为可选调试字段补回。

#### Broker 路由优先级

1. `X-Creavor-Upstream`
2. `X-Creavor-Session-Id` 对应的本地注册信息
3. 路径中的协议族（如 `/v1/openai/...`、`/v1/anthropic/...`）
4. `upstream[runtime]` 默认上游
5. `key_routing`（仅 fallback）

#### Broker 转发要求

- Broker 在转发到真实上游前，必须剥离所有 `X-Creavor-*` Header，避免内部控制头泄露给外部 API。
- 审计日志中可保留这些 Header 的脱敏副本，用于排障和归因。

---


## Support Levels（新增）

为避免不同工具的成熟度被误解为一致，Broker 对接入工具按支持等级分层：

| 级别 | 工具 | 说明 |
|------|------|------|
| **Tier 1** | Claude Code、OpenCode | 已有比较明确、稳定的代理接入路径；P0 重点支持 |
| **Tier 2** | Codex、Qwen Code | 能接入 Broker，但前置条件和边界更多；建议在 P0.5/P1 落地 |
| **Experimental** | Gemini CLI | 仅做实验性接入，不承诺企业生产可用 |

### 分级含义

- **Tier 1**：文档与实现均以此为主线；支持优先、问题优先修复。
- **Tier 2**：需要额外前置条件验证，或存在认证/配置边界；默认不作为第一批试点核心对象。
- **Experimental**：仅做 smoke test 级验证；不承诺配置稳定性、企业级安装体验或生产 SLA。

## Broker Core（新增）

本章描述与具体工具无关的 Broker 核心能力。所有 runtime 章节都建立在这一层之上。

### 1. 本地存储（SQLite）

Broker 默认使用本机 SQLite 持久化审计数据。P0 的目标不是“保存所有原始数据”，而是“在尽量不扩大泄露面的前提下，记录可审计、可排障、可归因的信息”。

#### 1.1 建议表结构

**events 表：记录运行时事件流（如 hook、会话事件）**

```sql
CREATE TABLE events (
    id              TEXT PRIMARY KEY,
    session_id      TEXT,
    runtime         TEXT NOT NULL,
    event_type      TEXT NOT NULL,
    payload         TEXT NOT NULL,
    created_at      TEXT NOT NULL
);
```

**requests 表：记录 API 请求流**

```sql
CREATE TABLE requests (
    id                  TEXT PRIMARY KEY,
    session_id          TEXT,
    runtime             TEXT,
    upstream_id         TEXT,
    method              TEXT NOT NULL,
    protocol_family     TEXT NOT NULL,
    request_path        TEXT NOT NULL,
    upstream_url        TEXT,
    request_headers     TEXT,
    request_summary     TEXT,
    response_status     INTEGER,
    response_summary    TEXT,
    blocked             INTEGER NOT NULL DEFAULT 0,
    block_reason        TEXT,
    rule_id             TEXT,
    severity            TEXT,
    latency_ms          INTEGER,
    created_at          TEXT NOT NULL
);
```

**request_payloads 表：按需保存脱敏后的请求体**

```sql
CREATE TABLE request_payloads (
    request_id   TEXT PRIMARY KEY,
    body         TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    FOREIGN KEY(request_id) REFERENCES requests(id)
);
```

**violations 表：记录命中的规则**

```sql
CREATE TABLE violations (
    id              TEXT PRIMARY KEY,
    request_id      TEXT,
    session_id      TEXT,
    runtime         TEXT,
    upstream_id     TEXT,
    rule_id         TEXT NOT NULL,
    rule_name       TEXT NOT NULL,
    severity        TEXT NOT NULL,
    matched_content TEXT,
    action          TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    FOREIGN KEY(request_id) REFERENCES requests(id)
);
```

#### 1.2 P0 存储原则

- **默认不保存完整响应体**。
- **默认只保存脱敏后的请求摘要**，而不是完整原文。
- **完整请求体仅在显式开启 `store_request_payloads=true` 时保存，且保存前必须先脱敏**。
- **SQLite 文件权限建议为 600**，目录权限建议为 700。

#### 1.3 并发写入建议：默认启用 SQLite WAL

Broker 会同时接收：
- API 请求流审计
- hook / 会话事件流
- Guard 审批动作写入

为降低多写场景下的锁冲突，建议 Broker 启动时默认执行：

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
```

说明：
- **WAL 模式**：提升读写并发能力，减少 `database is locked`
- **`busy_timeout`**：避免短时写冲突直接报错
- **短事务**：审计写入、审批动作写入都应尽量保持短事务，不要在一个事务里做长时间计算或网络请求

### 2. Redaction 默认策略

Broker 本身是防泄露组件，不能因为审计而成为新的泄露面。P0 默认使用 **mask** 策略：

- 匹配到的 secret / key / token / 密码片段 → `***` 或前后保留少量字符
- 邮箱、手机号、证件号等 PII → 部分脱敏
- headers 只保留白名单字段

建议默认值：

```toml
[audit]
store_request_payloads = false
store_response_payloads = false
sanitize_mode = "mask"
header_whitelist = ["content-type", "x-request-id", "x-ratelimit-remaining"]
```

### 3. CLI 命令

P0 的管理面优先通过 CLI 提供，不要求先做 Web 控制台。

```bash
creavor-broker start --config ~/.config/creavor/config.toml
creavor-broker status
creavor-broker stop
creavor-broker logs --last 24h
creavor-broker logs --blocked-only
creavor-broker rules list
creavor-broker rules test --input "sk-ant-xxxxx"
creavor-broker doctor
creavor-broker cleanup
creavor-guard start
```

#### 3.1 命令职责

- `start`：启动本地 Broker 服务
- `status`：查看端口、PID、SQLite 路径、规则加载状态
- `logs`：查询本地审计记录
- `rules list/test`：查看和验证规则
- `doctor`：检查配置、端口、上游连通性、证书、文件权限
- `cleanup`：恢复工具侧被修改的配置文件，清理残留状态
- `creavor-guard start`：启动本地 Guard MCP server，供 Claude Code 中的交互审批调用

### 4. 配置文件结构

Broker 使用单一主配置文件，例如：

```toml
[broker]
port = 8765
log_level = "info"
block_status_code = 400
block_error_style = "auto"

[audit]
db_path = "~/.local/share/creavor/broker.db"
retention_days = 90
store_request_payloads = false
store_response_payloads = false
sanitize_mode = "mask"
header_whitelist = ["content-type", "x-request-id", "x-ratelimit-remaining"]

[rules]
rules_dir = "~/.config/creavor/rules"
builtin_secrets = true
builtin_pii = true
builtin_key_patterns = true

[sync]
enabled = false
central_url = ""
central_token = ""
sync_interval = "5m"

[llm_analyzer]
enabled = false

[guard]
approval_timeout_secs = 60
default_timeout_action = "block"
```

上游信息建议放在 Broker 本地注册表中，而不是完全依赖客户端 Header 直接决定真实目标。

### 5. Provider-compatible Error Responses

Broker 阻断请求时，应尽量返回调用方 SDK/工具更容易理解的错误格式，而不是统一返回 Creavor 自定义 JSON。

#### 5.1 Anthropic 风格

```json
{
  "type": "error",
  "error": {
    "type": "invalid_request_error",
    "message": "Request blocked by security policy"
  }
}
```

#### 5.2 OpenAI 风格

```json
{
  "error": {
    "message": "Request blocked by security policy",
    "type": "invalid_request_error",
    "code": "content_policy_violation"
  }
}
```

#### 5.3 Gemini 风格

Gemini CLI 当前仍定位为 experimental，但 Broker 仍应补齐**最小兼容错误体**，避免因为错误返回格式过于随意导致 SDK / CLI 行为异常。

建议 P0 先返回最小 Gemini 风格错误结构：

```json
{
  "error": {
    "code": 400,
    "message": "Request blocked by security policy",
    "status": "INVALID_ARGUMENT"
  }
}
```

建议：
- 默认 HTTP 状态码为 `400`
- 对高危阻断可配置为 `403`
- 审计日志中继续保存内部字段：
  - `rule_id`
  - `severity`
  - `matched_content`（脱敏后）
- 若后续 smoke test 发现 Gemini CLI 对错误结构还有额外要求，再补充更高保真度的兼容实现



### 6. 交互审批与 Creavor Guard（新增）

对于“用户输入阶段”和“最终 API 请求阶段”的风险处理，Broker 采用分层策略：

#### 6.1 风险分级

| 风险级别 | 默认动作 | 是否允许用户自行放行 |
|---------|---------|----------------------|
| `critical` | 直接阻断 | 否 |
| `high` | 先阻断，进入待审批 | 是（受策略控制） |
| `medium` | 先阻断，进入待审批 | 是 |
| `low` | 默认放行并记录，可弱提示 | 一般不需要 |

#### 6.2 组件边界

为避免把交互式审批逻辑塞进 Broker 主进程，审批交互组件单独命名为 **Creavor Guard**。结合当前真实仓库结构：

```text
opencreavor/
├── apps/
│   ├── broker/
│   ├── guard/
│   └── creavor-cli/
└── libs/
    └── creavor-core/
```

其中：

- `apps/broker/`
  - 请求代理
  - 规则检测
  - 风险分级
  - 审计落库
  - block / allow 执行
  - 待审批请求缓存
- `apps/guard/`
  - Guard Core + 面向工具的交互审批入口
  - Claude Code 场景下提供 MCP server / elicitation 交互
  - 后续可扩展 OpenCode / Codex 的审批桥接
  - 展示脱敏摘要
  - 把审批结果回写 Broker
- `apps/creavor-cli/`
  - 保留各工具的安装、配置、run 子命令
  - 继续在 `src/runtimes/*.rs` 中维护工具特定接入逻辑
  - 例如 `creavor run claude`、`creavor run opencode`、`creavor run codex`
- `libs/creavor-core/`
  - 放共享的配置读取、运行时枚举、通用类型
  - 作为 Broker / Guard / CLI 三者共用类库

**设计原则：**
- Guard 的**核心能力**放 `apps/guard/`，不要散落到各个工具目录中重复实现。
- 各工具的**接入差异**仍然留在 `apps/creavor-cli/src/runtimes/`，由 CLI 负责配置注入、启动参数和恢复逻辑。
- 共享配置与基础类型优先放到 `libs/creavor-core/`，避免在 Broker 和 Guard 中重复维护。

#### 6.2.1 建议的 Guard 目录结构

在当前工程中，推荐新增：

```text
apps/
  guard/
    ├── Cargo.toml
    ├── config/
    │   └── guard.example.json
    ├── src/
    │   ├── main.rs                # guard 进程入口
    │   ├── lib.rs
    │   ├── app.rs                 # guard 启动与路由装配
    │   ├── approval.rs            # 审批状态机：allow once / allow session / block
    │   ├── broker_client.rs       # 与 broker 本地 API 通信
    │   ├── policy.rs              # 风险级别与审批策略
    │   ├── prompt.rs              # 脱敏摘要与展示文案生成
    │   ├── mcp/
    │   │   ├── mod.rs
    │   │   ├── server.rs          # Claude Code MCP server
    │   │   ├── tools.rs           # review_pending / list_pending / show_summary
    │   │   └── elicitation.rs     # 交互式放行/阻断选项
    │   ├── adapters/
    │   │   ├── mod.rs
    │   │   ├── claude_code.rs     # Claude Code 专用桥接
    │   │   ├── opencode.rs        # OpenCode 审批桥接（question/plugin）
    │   │   └── codex.rs           # Codex 审批桥接（MCP/App Server）
    │   └── settings.rs            # guard 自身配置
    └── tests/
        ├── approval_flow.rs
        ├── broker_client.rs
        └── mcp_elicitation.rs
```

#### 6.2.2 `libs/creavor-core/` 建议补充的共享内容

当前 `libs/creavor-core/` 已放配置读取等通用能力，建议继续放这些**跨 app 共享**内容：

- `settings.rs`
  - 读取 opencreavor 全局配置
  - Broker / Guard / CLI 共用路径与端口配置
- `runtime.rs`
  - 统一的 runtime 枚举：`claude-code`、`opencode`、`codex`、`qwen-code`、`gemini-cli`
- 新增建议：`contracts.rs` 或 `guard.rs`
  - `RiskLevel`
  - `ApprovalDecision`
  - `ApprovalRequestRef`
  - `UpstreamId`
  - `SessionId`

这样分层后：
- Broker 不需要知道 Claude/OpenCode/Codex 的 UI 细节
- Guard 不需要自己重新实现配置读取
- CLI 继续负责“把每个工具接到 Broker/Guard 上”，而不是承载审批核心逻辑

#### 6.3 Claude Code 中的交互审批路径

P0 推荐采用 **Broker + Guard(MCP)** 的组合，而不是自定义外壳窗口：

1. Claude Code 发出最终 API 请求到 Broker
2. Broker 检测真实出站请求
3. 若为 `critical`
   - 直接阻断
   - 返回 provider-compatible error
   - 记录审计
4. 若为 `high` / `medium`
   - Broker 不立即转发
   - 创建 `pending approval`
   - Guard 通过 Claude Code 中的 MCP Elicitation 向用户展示选项
5. 用户选择：
   - `Allow once`
   - `Allow for this session`
   - `Block`
   - （可选）`View sanitized details`
6. Guard 将审批结果回写 Broker
7. Broker 记录审计并执行一次性放行或继续阻断

> 说明：用户输入阶段的高置信度敏感信息，仍优先通过 `UserPromptSubmit` hook 直接阻断；Guard 主要用于“Broker 检测到最终完整请求存在中/高风险，需要用户交互审批”的场景。

#### 6.4 建议新增表结构

**approval_requests 表：记录待审批请求**

```sql
CREATE TABLE approval_requests (
    id                  TEXT PRIMARY KEY,
    request_id          TEXT NOT NULL,
    session_id          TEXT,
    runtime             TEXT NOT NULL,
    upstream_id         TEXT,
    risk_level          TEXT NOT NULL,
    rule_id             TEXT NOT NULL,
    sanitized_summary   TEXT NOT NULL,
    status              TEXT NOT NULL,   -- pending / approved / rejected / expired
    expires_at          TEXT,
    created_at          TEXT NOT NULL,
    FOREIGN KEY(request_id) REFERENCES requests(id)
);
```

**approval_actions 表：记录用户审批动作**

```sql
CREATE TABLE approval_actions (
    id                  TEXT PRIMARY KEY,
    approval_request_id TEXT NOT NULL,
    action              TEXT NOT NULL,   -- allow_once / allow_session / block
    actor               TEXT NOT NULL,   -- local_user
    source              TEXT NOT NULL,   -- guard_mcp
    created_at          TEXT NOT NULL,
    FOREIGN KEY(approval_request_id) REFERENCES approval_requests(id)
);
```

#### 6.5 Approval Timeout（新增）

为避免用户迟迟不审批导致请求永久挂起、阻塞工具，Guard 必须为 `pending approval` 引入超时机制。

建议默认行为：
- `approval_timeout_secs = 60`
- 超时后默认动作：`block`
- Guard / Broker 返回明确提示：
  - `[Creavor Guard] Approval timed out, request blocked`
- 审计表中记录：
  - `status = expired`
  - `expires_at`
  - `action = timeout_block`

实现要求：
- Broker 在创建 `approval_requests` 时写入 `expires_at`
- Guard 在读取待审批项时过滤已过期请求
- 过期请求不允许再被 `allow once` 或 `allow for this session` 激活
- CLI 后续可增加清理命令，用于清除历史 `expired` 审批项

#### 6.6 审计要求

以下动作全部必须进入本地 SQLite 审计：

- 用户输入阶段被 hook 阻断
- Broker 阶段命中规则并生成待审批
- 用户在 Guard 中选择 `Allow once`
- 用户在 Guard 中选择 `Allow for this session`
- 用户明确选择 `Block`
- `critical` 风险自动强制阻断

这样后续审计可以回答：
- 哪些请求被拦截
- 哪些被用户自行放行
- 放行的是哪一类风险
- 是一次性放行还是会话级放行


## Path Rewrite & URL Join Rules（新增）

Broker 对外暴露的是统一本地协议路径；真实上游的原生 base URL 由 Broker 内部维护。由于两者通常不同，Broker 必须做**前缀剥离 + 尾部拼接**，而不是把本地 path 原样透传。

### 1. 统一规则

假设本地请求路径为：

```text
/v1/{protocol}/{upstream-id}/{tail}
```

则 Broker 的转发规则为：

```text
upstream_base_url = registry[upstream-id].base_url
forward_url = normalize_join(upstream_base_url, tail)
```

其中：
- `protocol` 仅用于本地入口分组和校验，不直接决定真实 URL
- `upstream-id` 用于定位 Broker 本地注册表中的目标上游
- `tail` 是上游真正要访问的 API 路径片段
- `normalize_join()` 负责处理重复 `/`、重复 `v1`、保留 query string 等问题

### 1.1 Trailing Slashes 容错（新增）

为避免因为 `.../v1/` 与 `.../v1` 拼接不一致导致 404，Broker 必须在拼接前先做标准化：

```text
normalized_base_url:
  - 去掉末尾多余 `/`
normalized_tail:
  - 保证以单个 `/` 开头
  - 保留原始 query string
forward_url:
  - normalized_base_url + normalized_tail
```

建议实现约束：
- 不允许直接用字符串裸拼接
- 必须统一走 `normalize_join(base_url, tail)`
- `normalize_join()` 需要处理：
  - `https://api.xxx.com/v1` + `/responses`
  - `https://api.xxx.com/v1/` + `/responses`
  - `https://open.bigmodel.cn/api/anthropic/` + `/messages`
  - `https://generativelanguage.googleapis.com/` + `/models/...`

### 2. Anthropic 示例

```text
incoming path: /v1/anthropic/zhipu-anthropic/messages
registry base_url: https://open.bigmodel.cn/api/anthropic
forward url: https://open.bigmodel.cn/api/anthropic/messages
```

### 3. OpenAI 示例

```text
incoming path: /v1/openai/openai-direct/responses
registry base_url: https://api.openai.com/v1
forward url: https://api.openai.com/v1/responses
```

### 4. Gemini 示例

```text
incoming path: /v1/gemini/google-direct/models/gemini-2.5-pro:generateContent
registry base_url: https://generativelanguage.googleapis.com
forward url: https://generativelanguage.googleapis.com/models/gemini-2.5-pro:generateContent
```

### 5. Claude Code 的简化路径

Claude Code P0 仍可继续使用：

```text
/v1/anthropic/messages
```

此时 `upstream-id` 不从 path 中显式传递，而是优先从：
1. `X-Creavor-Upstream`
2. `X-Creavor-Session-Id` 绑定的本地注册信息
3. runtime 默认 upstream

解析得到，再执行同样的 `normalize_join(upstream_base_url, tail)`。


## 第一章：Claude Code

### 1.1 运行时信息

| 项目 | 路径/方式 |
|------|----------|
| 配置文件 | `~/.claude/settings.json` |
| API 地址字段 | `env.ANTHROPIC_BASE_URL` |
| API Key 字段 | `env.ANTHROPIC_AUTH_TOKEN` 或环境变量 `ANTHROPIC_API_KEY` |
| 环境变量覆盖 | `ANTHROPIC_BASE_URL`（设置后覆盖配置文件） |
| 自定义 Headers | `ANTHROPIC_CUSTOM_HEADERS` |

### 1.2 拦截方式

Claude Code 通过环境变量 `ANTHROPIC_BASE_URL` 控制 API 请求地址。`creavor run claude` 的流程：

```
1. 读取 ~/.claude/settings.json → env.ANTHROPIC_BASE_URL
   例: https://open.bigmodel.cn/api/anthropic

2. 保存为 upstream:
   settings.json → upstream.claude-code = "https://open.bigmodel.cn/api/anthropic"

3. 根据原始 upstream URL 匹配或注册 Broker 上游 ID:
   https://open.bigmodel.cn/api/anthropic → upstream_id = zhipu-anthropic

4. 设置环境变量启动:
   ANTHROPIC_BASE_URL=http://127.0.0.1:8765/v1/anthropic
   ANTHROPIC_CUSTOM_HEADERS=X-Creavor-Session-Id:<sid>,X-Creavor-Runtime:claude-code,X-Creavor-Upstream:zhipu-anthropic
   CREAVOR_SESSION_ID=<sid>

5. Claude Code 发请求:
   POST http://127.0.0.1:8765/v1/anthropic/messages
   Headers: x-api-key: <key>, x-creavor-runtime: claude-code, x-creavor-upstream: zhipu-anthropic, x-creavor-session-id: <sid>

6. Broker 收到请求:
   路径 /v1/anthropic/messages → protocol family = anthropic
   header x-creavor-runtime=claude-code → runtime=claude-code
   header x-creavor-upstream=zhipu-anthropic → upstream_id=zhipu-anthropic
   查 upstream_registry["zhipu-anthropic"] → https://open.bigmodel.cn/api/anthropic
   转发到: https://open.bigmodel.cn/api/anthropic/messages
```

### 1.2.1 `session_id` 与 plugin/hooks 采集

Claude Code 场景下，`session_id` 的最佳来源不是由 Broker 自己猜，而是由 **Claude Code 官方 hooks / plugin 事件** 提供。推荐方式：

- 在 `SessionStart` 事件中获取 Claude 官方 `session_id`
- 将 `session_id`、`cwd`、`transcript_path`、启动时间等信息通过本地 HTTP 上报给 Broker
- 后续 `UserPromptSubmit`、`PreToolUse`、`PostToolUse`、`SessionEnd` 继续带同一 `session_id` 上报

这样 Broker 可以用“事件流”建立会话，再把后续 API 请求关联到该会话。

> 说明：plugin/hooks **很适合做事件采集与会话注册**，但 P0 不建议把“给每个 API 请求稳定注入 `X-Creavor-Session-Id`”完全依赖在 plugin 上。
> 原因是：静态配置容易注入 `runtime` / `upstream`，而 `session_id` 是运行时动态值。若后续验证 Claude hooks 能稳定把动态值传入后续请求 header，再可升级为更强方案。

因此，Claude Code 在 P0 推荐分两层：

1. **请求层**：通过 `ANTHROPIC_BASE_URL` + `ANTHROPIC_CUSTOM_HEADERS` 让请求经过 Broker，并静态携带 `runtime` / `upstream`
2. **事件层**：通过 hooks/plugin 把 Claude 官方 `session_id` 与会话事件发给 Broker

如果某次请求没有显式 `X-Creavor-Session-Id`，Broker 仍可通过：
- `runtime=claude-code`
- `upstream_id`
- 时间窗口
- 最近活跃的 open session

进行 best-effort 关联。



### 1.2.2 Claude Code 中的拦截提示与交互放行

Claude Code 的提示与放行流程分成两层：

#### A. 用户输入阶段（hook 直接处理）
- 使用 `UserPromptSubmit` hook 做预检
- 对高置信度敏感内容可直接 `block`
- 理由直接显示在 Claude 当前对话中
- 该阶段不需要 Guard 参与

#### B. 最终 API 请求阶段（Broker + Guard）
- Broker 检测 Claude 最终发出的完整请求
- `critical`：直接阻断，返回兼容 Anthropic 的错误体
- `high` / `medium`：Broker 生成待审批记录，由 **Creavor Guard** 发起交互审批

#### Guard 的交互形态

Creavor Guard 作为独立 MCP server 运行在 `apps/guard/`，在 Claude Code 中提供交互式审批能力：

- 通过 MCP Elicitation 弹出选项
- 展示脱敏后的风险摘要
- 用户可选择：
  - `Allow once`
  - `Allow for this session`
  - `Block`

这样实现后：
- **有拦截时**：用户在 Claude Code 中能看到统一的审批/阻断体验
- **没拦截时**：与普通 Claude Code 基本一致
- **不需要外壳窗口**：P0 不引入自定义终端壳

#### 与 `session_id` 的关系

- `session_id` 主要用于把 Claude hooks 事件流与 Broker 请求流关联
- Guard 的审批结果也应尽量带上 `session_id`
- 如果某次请求未能稳定注入 `X-Creavor-Session-Id`，Broker 仍可使用 `runtime + upstream + timestamp window` 做回退关联


### 1.3 是否支持运行中切换供应商

**否。** Claude Code 只支持一个上游 API 地址。

验证依据：
- 配置文件 `~/.claude/settings.json` 中只有一组 `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN`
- 支持的 3P provider（Bedrock/Vertex/Foundry）通过 `--settings` 或 `--bare` 启动参数指定，**启动时确定，运行中不可切换**
- `--model` 参数可切换模型（如 opus → sonnet），但**不切换供应商**，所有模型走同一个 `ANTHROPIC_BASE_URL`
- `settings.json` 中有 `192_ANTHROPIC_AUTH_TOKEN`（前缀 `192_` 看起来是旧配置），但同一时间只有一个生效

结论：**不需要动态路由方案**。单一 upstream，启动时确定。

### 1.4 当前实现状态

**已完成（P0 主路径）**。`creavor run claude` 已实现：
- 自动读取并保存原始 upstream URL
- 通过环境变量注入代理地址
- 通过 `ANTHROPIC_CUSTOM_HEADERS` 传递统一 Header（至少 `runtime/upstream`）

**待补充（推荐增强）**：
- 通过 hooks/plugin 上报 Claude 官方 `session_id`
- 将事件流与 API 请求流在 Broker 侧关联
- 将 `X-Creavor-Session-Id` 保持为推荐字段，而不是 Claude P0 的硬依赖

---

## 第二章：OpenCode

### 2.1 运行时信息

| 项目 | 路径/方式 |
|------|----------|
| 全局配置文件 | `~/.config/opencode/opencode.jsonc`（常见）或 `~/.config/opencode/opencode.json` |
| 旧版兼容路径 | `~/.local/share/opencode/opencode.jsonc`（旧版本可能存在） |
| 认证存储 | `~/.local/share/opencode/auth.json` |
| Provider 配置入口 | `provider.<id>.options.*` |
| Base URL 覆盖字段 | `provider.<id>.options.baseURL` |
| 自定义 Headers | `provider.<id>.options.headers` |
| HTTP API | `opencode serve` 默认端口 4096，可通过 `/config/providers` 观察 provider 信息 |

> 说明：本章设计以 **`provider.*.options.baseURL` + 可选 `headers`** 为主方案；`OPENAI_BASE_URL` 不是 OpenCode 的通用覆盖入口，不能作为 P0 方案依赖。

### 2.1.1 JSONC 解析与保注释写回（新增）

OpenCode 的全局配置文件在现实中经常是 `jsonc`，用户可能自行写了注释。  
因此 Creavor 不应使用“纯 JSON 解析 + 重新序列化整文件”的方式改写配置，否则会擦除注释与部分格式信息。

P0 建议：
- 优先按 **JSONC** 解析 `opencode.jsonc`
- 若是 `opencode.json`，按普通 JSON 解析
- 写回时仅做**增量字段修改**
- 尽量保留用户注释、字段顺序和其他无关格式
- 如果当前实现暂时不能完整保格式，也要在 `doctor` 或日志里明确提示“注释可能丢失”

### 2.2 供应商体系

OpenCode 同时支持：

1. **内置 provider**（二进制内置，用户通过 `auth.json` 或 `/connect` 补充凭证）
2. **自定义 provider**（用户可在配置文件中定义任意 upstream ID、npm 适配器、baseURL、models）

因此，Broker 方案的关键不是“劫持所有内置 provider”，而是：

- **优先覆盖当前启用/已配置的 provider**
- 让这些 provider 的请求都指向 Broker
- 由 Broker 根据路径或 header 确定目标上游

认证信息仍然存放在 `auth.json`，例如：

```json
{
  "zhipuai-coding-plan": {
    "type": "api",
    "key": "76b5b2cf51c74aef8872bf08a12a75ae.BhqBO3k1QqW3n5VL"
  }
}
```

### 2.3 P0 拦截方式（推荐主方案）

OpenCode 的 P0 不采用“所有 provider 共享一个 `/v1/openai` 再靠 API Key 猜上游”的方案。  
**推荐主方案是：每个 provider 使用独立的 Broker 路径，必要时附带显式 header。**

#### 配置示例：覆盖已有 provider

```json
// ~/.config/opencode/opencode.jsonc 或 opencode.json
{
  "provider": {
    "zhipuai-coding-plan": {
      "options": {
        "baseURL": "http://127.0.0.1:8765/v1/openai/zhipuai-coding-plan",
        "headers": {
          "X-Creavor-Runtime": "opencode",
          "X-Creavor-Upstream": "zhipuai-coding-plan"
        }
      }
    },
    "zai": {
      "options": {
        "baseURL": "http://127.0.0.1:8765/v1/openai/zai",
        "headers": {
          "X-Creavor-Runtime": "opencode",
          "X-Creavor-Upstream": "zai-openai"
        }
      }
    },
    "anthropic": {
      "options": {
        "baseURL": "http://127.0.0.1:8765/v1/anthropic/anthropic",
        "headers": {
          "X-Creavor-Runtime": "opencode",
          "X-Creavor-Upstream": "anthropic-direct"
        }
      }
    }
  }
}
```

> 说明：OpenCode 的 `options.headers` 适合承载 **静态且确定的 Header**（如 `X-Creavor-Runtime`、`X-Creavor-Upstream`）。
> `X-Creavor-Session-Id` 属于运行时动态值，P0 不建议把它写死在配置文件里；若后续 OpenCode 插件/启动器支持动态注入，可再补齐。当前 P0 可接受 OpenCode 仅显式携带 `runtime + upstream`，会话关联依赖请求流时间窗、事件流或 fallback。

#### 请求路径示意

```
用户选 zhipuai-coding-plan:
  POST http://127.0.0.1:8765/v1/openai/zhipuai-coding-plan/chat/completions

用户切换到 zai:
  POST http://127.0.0.1:8765/v1/openai/zai/chat/completions

用户切换到 anthropic:
  POST http://127.0.0.1:8765/v1/anthropic/anthropic/messages
```

Broker 收到请求后，不再需要先猜“这是哪个供应商”，而是直接从：

- 路径中的 upstream ID，或
- `X-Creavor-Upstream` header

得到精确目标。

#### 为什么这是 P0 最佳方案

- **简单**：不需要 API Key 前缀路由作为主路径
- **稳定**：不依赖“不同供应商 key 一定能稳定区分”
- **可解释**：路径本身就表达了 runtime + provider
- **支持运行中切换**：OpenCode 切换 provider 时，会自然使用该 provider 自己的 `baseURL`
- **兼容自定义 provider**：新增 provider 只需新增配置项，不必维护庞大的内置映射表

### 2.4 可选增强：自定义 Creavor Provider（P1）

除了覆盖已有 provider，还可以在 OpenCode 中新增自定义 provider，例如：

```json
{
  "provider": {
    "creavor-openai": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Creavor OpenAI Gateway",
      "options": {
        "baseURL": "http://127.0.0.1:8765/v1/openai/creavor-openai",
        "apiKey": "{env:CREAVOR_OPENAI_KEY}"
      },
      "models": {
        "gpt-4o": {},
        "glm-5": {},
        "deepseek-chat": {}
      }
    }
  }
}
```

这条路的优点是“用户明确知道自己在使用 Creavor provider”；缺点是：
- 需要额外维护模型列表
- 用户的 provider 选择体验会发生变化

因此建议：
- **P0：优先覆盖已有 provider**
- **P1：再评估是否引入自定义 Creavor provider**

### 2.5 配置文件安全：合并写入与增量恢复

用户可能已有自己的 `opencode.jsonc/json`，Creavor 必须**合并**而非覆盖。

#### 写入规则

```
1. 只修改 provider.*.options.baseURL 和（可选）provider.*.options.headers
2. provider.*.options 的其他字段保留用户原有值
3. 其他顶层字段（theme、mcp、plugin 等）完全不动
```

#### 恢复规则

```
1. 不还原整个文件，只回填被修改的字段
2. baseURL 恢复为原始值
3. headers 只回滚 Creavor 写入的 header 项（如 X-Creavor-Runtime / X-Creavor-Upstream）
4. 用户在运行期间对其他字段的修改不受影响
```

#### 变更清单示例

```json
{
  "file": "~/.config/opencode/opencode.jsonc",
  "changes": [
    { "path": "provider.zhipuai-coding-plan.options.baseURL", "old": null },
    { "path": "provider.zhipuai-coding-plan.options.headers.X-Creavor-Runtime", "old": null },
    { "path": "provider.zhipuai-coding-plan.options.headers.X-Creavor-Upstream", "old": null },
    { "path": "provider.zai.options.baseURL", "old": "https://open.bigmodel.cn/api/paas/v4" }
  ]
}
```

#### 崩溃恢复机制

```
启动时:
  1. 记录被修改字段的原始值（变更清单）
  2. 将变更清单写入 ~/.config/opencode/opencode.jsonc.creavor-changes.<timestamp>
  3. 合并写入 baseURL / headers

正常退出:
  1. 读取最新的 .creavor-changes.<timestamp>
  2. 遍历 changes 回填 old 值
     - old 为 null → 删除该字段
     - old 有值 → 恢复为 old 值
  3. 删除 .creavor-changes 文件

异常退出 (kill -9):
  1. 下次启动时扫描 .creavor-changes.* 文件
  2. 按时间戳排序依次恢复
  3. 清理残留文件
  4. 然后进入新的启动流程
```

#### `creavor cleanup` 手动恢复

```
用法:
  creavor cleanup
  creavor cleanup opencode

逻辑:
  1. 在 OpenCode 配置目录扫描 .creavor-changes.* 文件
  2. 按时间戳排序，依次恢复变更
  3. 删除 .creavor-changes 文件
  4. 报告恢复结果
```

### 2.6 是否支持运行中切换供应商

**是。** OpenCode 允许用户在运行中切换 provider / model。

但在新的 Broker 方案中，这不再是“难点”，因为：

- 每个 provider 有自己的 `baseURL`
- 每个 provider 也可有自己的 `headers`
- OpenCode 切换 provider 后，请求自然走该 provider 对应的 Broker 路径

因此，**P0 不需要把“API Key 前缀动态路由”作为主方案**。

### 2.7 备用方案：API Key 前缀路由（仅作为 fallback）

只有在下列场景下，才启用 API Key 前缀路由作为兜底：

- 某些 provider 无法单独设置独立 Broker 路径
- 请求到达 Broker 时缺少 `X-Creavor-Upstream`
- 某些旧配置只把所有 OpenAI-compatible provider 都指向了同一个 `/v1/openai` 入口

此时 Broker 可用：

- `Authorization: Bearer <key>`
- `auth.json` 中的 provider → key 关系

做“**best effort**” 的 key prefix 路由。

**但要强调：**
- 这是 **fallback**
- 不是 P0 主路径
- 不能作为 OpenCode 设计的核心前提

### 2.8 当前实现状态

**需要修订实现。** 现阶段 `creavor run opencode` 若仍通过 `OPENAI_BASE_URL` 或 `provider.*.options.endpoint` 注入代理地址，应视为旧方案。

新的实现任务应调整为：

- [ ] 兼容读取 `opencode.jsonc` / `opencode.json`
- [ ] 通过 `provider.*.options.baseURL` 注入 Broker 地址
- [ ] 支持按 provider 生成不同的 Broker 路径
- [ ] 可选注入 `provider.*.options.headers`（统一 Header：`X-Creavor-Runtime` / `X-Creavor-Upstream`）
- [ ] 合并写入（仅修改 `baseURL` / 指定 headers）
- [ ] 增量恢复机制（变更清单 + 时间戳备份 + 崩溃自动恢复）
- [ ] `creavor cleanup opencode`
- [ ] API Key 前缀路由仅保留为 fallback


---

## 第三章：Codex

### 3.1 运行时信息

| 项目 | 路径/方式 |
|------|----------|
| 用户级配置文件 | `~/.codex/config.toml` |
| 项目级配置文件 | `.codex/config.toml` |
| 自定义 provider | `model_provider` + `[model_providers.<id>]` |
| Provider Base URL | `model_providers.<id>.base_url` |
| 静态自定义 Headers | `model_providers.<id>.http_headers` |
| 动态环境 Header | `model_providers.<id>.env_http_headers` |
| 本地/开源模式 | `--oss` 或 `oss_provider` |

### 3.2 官方能力判断

Codex 的 P0 设计应基于 **`config.toml` 的自定义 model provider**，而不是假设 `OPENAI_BASE_URL` 这种环境变量覆盖入口。

官方文档已经明确支持：

- 在 `~/.codex/config.toml` 中定义自定义 provider
- 用 `base_url` 指向代理或自定义模型服务
- 用 `http_headers` / `env_http_headers` 注入额外请求头
- 通过 `model_provider` 选择该 provider

因此，Codex 不应再被视为“只能绑定 ChatGPT OAuth、无法接第三方代理”的工具。P0 的主方案应是 **配置文件方式接入 Broker**。

### 3.3 P0 拦截方式（推荐主方案）

推荐在 `~/.codex/config.toml` 写入或合并如下配置：

```toml
model = "gpt-5.4"
model_provider = "creavor-openai"

[model_providers.creavor-openai]
name = "Creavor OpenAI Broker"
base_url = "http://127.0.0.1:8765/v1/openai/openai-direct"
env_key = "OPENAI_API_KEY"
http_headers = { "X-Creavor-Runtime" = "codex", "X-Creavor-Upstream" = "openai-direct" }
env_http_headers = { "X-Creavor-Session-Id" = "CREAVOR_SESSION_ID" }
```

如果实际目标不是 OpenAI 第一方，而是企业中心网关或兼容服务，则仅需替换：

```toml
base_url = "http://127.0.0.1:8765/v1/openai/<upstream-id>"
http_headers = { "X-Creavor-Runtime" = "codex", "X-Creavor-Upstream" = "<upstream-id>" }
```

#### 请求路径示意

```
Codex 发请求:
  POST http://127.0.0.1:8765/v1/openai/openai-direct/responses
  Authorization: Bearer <token or key>
  X-Creavor-Runtime: codex
  X-Creavor-Upstream: openai-direct
  X-Creavor-Session-Id: <sid>   (如果 env_http_headers 已注入)

Broker 路由:
  1. 读取 x-creavor-upstream=openai-direct
  2. 查 upstream_registry["openai-direct"]
  3. 转发到真实上游
```

### 3.4 是否支持运行中切换供应商

**支持，但推荐视作“配置切换”，而不是 OpenCode/Qwen 那种运行中多 provider 动态切换。**

Codex 官方支持：

- 用 `model_provider` 指向不同 provider
- 在 `config.toml` 中预定义多个 provider
- 用 `-c` / `--config` 做单次运行覆盖
- 用 `--oss` 切换到本地 provider（如 Ollama / LM Studio）

因此，P0 可以认为：

- **支持多 provider**
- 但通常是“启动时确定本次使用哪个 provider”
- 不需要像 OpenCode 那样引入复杂的 provider 运行时猜测逻辑

### 3.5 当前实现状态

Codex 的 P0 实现建议改为：

- [ ] 读取并合并 `~/.codex/config.toml` / `.codex/config.toml`
- [ ] 注入 `model_provider = "creavor-..."`
- [ ] 注入 `[model_providers.<id>]` 的 `base_url`
- [ ] 注入 `http_headers`（`X-Creavor-Runtime` / `X-Creavor-Upstream`）
- [ ] 可选注入 `env_http_headers`（`X-Creavor-Session-Id`）
- [ ] 崩溃恢复与 `creavor cleanup codex`

---

## 第四章：Gemini CLI

### 4.1 运行时信息

| 项目 | 路径/方式 |
|------|----------|
| 安装方式 | npm / npx / Homebrew |
| API Key | `GEMINI_API_KEY`（常见） |
| Base URL 覆盖 | `GOOGLE_GEMINI_BASE_URL`（公开 issue 中确认可用，但文档成熟度较低） |
| Vertex Base URL | `GOOGLE_VERTEX_BASE_URL` |
| MCP 支持 | 支持 |
| Sandbox 影响 | 开启 sandbox 时，Base URL 环境变量可能不被传入子环境 |

### 4.2 官方能力判断

Gemini CLI 的代理接入能力 **目前不如 Claude Code / OpenCode / Codex 稳定**。

从公开资料看：

- 社区/issue 已确认可以通过 `GOOGLE_GEMINI_BASE_URL` 或 `GOOGLE_VERTEX_BASE_URL` 把流量导向 LiteLLM/代理
- 但这一路径在历史版本中存在 **sandbox 场景下环境变量不透传** 的问题
- 同时还有多个 issue 说明自定义 base URL 支持仍不够成熟

因此，Gemini CLI 在本方案中应定义为：

- **P0：实验支持（experimental）**
- **P1：待官方稳定后再升级为正式支持**

### 4.3 P0 拦截方式（实验支持）

推荐最小方案：

```bash
export GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:8765/v1/gemini/gemini-direct
export GEMINI_API_KEY=<原始 key>
export CREAVOR_SESSION_ID=<sid>

gemini
```

#### 请求路径示意

```
Gemini CLI 发请求:
  POST http://127.0.0.1:8765/v1/gemini/gemini-direct/models/gemini-2.5-pro:generateContent
  x-goog-api-key: <key>

Broker 路由:
  1. 路径前缀 /v1/gemini/gemini-direct
  2. upstream_id = gemini-direct
  3. 查 upstream_registry["gemini-direct"]
  4. 转发到真实 Google Gemini API
```

### 4.4 关键限制

1. **Base URL 能力当前更像公开验证过的实现入口，而不是非常成熟的官方主配置路径**。
2. **开启 sandbox 时，`GOOGLE_GEMINI_BASE_URL` 可能不被传入子环境**，导致请求回退到默认 Google 端点。
3. 因此在 P0 中：
   - 推荐先在 **无 sandbox** 或受控环境中验证
   - 将 Gemini CLI 标记为 **实验支持**
   - 不把它作为 Broker 首个必须打通的样板工具

### 4.5 当前实现状态

Gemini CLI 的 P0 实现建议为：

- [ ] 支持通过 `GOOGLE_GEMINI_BASE_URL` 指向 Broker
- [ ] 支持 `GOOGLE_VERTEX_BASE_URL`（如企业走 Vertex）
- [ ] 在 `creavor doctor gemini` 中明确检测环境变量是否生效
- [ ] 当 sandbox 开启时，提示“代理可能失效，需要 smoke test”
- [ ] 将 Gemini CLI 明确标记为 experimental

---

## 第五章：Qwen Code（通义灵码）

### 5.1 运行时信息

| 项目 | 路径/方式 |
|------|----------|
| 用户级配置文件 | `~/.qwen/settings.json` |
| 项目级配置文件 | `.qwen/settings.json` |
| 环境变量文件 | `.qwen/.env`（项目级）、`~/.qwen/.env`（用户级） |
| 核心配置入口 | `modelProviders` |
| 每模型 Base URL | `modelProviders.<protocol>[].baseUrl` |
| API Key 引用 | `envKey` |
| 模型切换 | `/model` |
| 启动协议选择 | `security.auth.selectedType` |

### 5.2 官方能力判断

Qwen Code 官方文档已经非常明确：

- `modelProviders` 的 key 代表协议（如 `openai`、`anthropic`、`gemini`）
- 每个模型条目都有独立的 `envKey`
- 可选 `baseUrl`，可用于代理或自定义 endpoint
- 运行时通过 `/model` 在不同 provider / model 之间切换
- 同一份 `settings.json` 可以混合配置多种协议

因此，Qwen Code 的 P0 方案不应依赖“修改全局 `OPENAI_BASE_URL` 再靠 API key 猜上游”，而应采用：

**每个模型条目独立改写 `baseUrl`，让路径直接表达 upstream。**

### 5.3 P0 拦截方式（推荐主方案）

推荐修改 `~/.qwen/settings.json` 中的 `modelProviders`：

```json
{
  "modelProviders": {
    "openai": [
      {
        "id": "qwen3-coder-plus",
        "name": "Qwen3 Coder Plus",
        "envKey": "DASHSCOPE_API_KEY",
        "baseUrl": "http://127.0.0.1:8765/v1/openai/dashscope-openai"
      },
      {
        "id": "gpt-4o",
        "name": "GPT-4o",
        "envKey": "OPENAI_API_KEY",
        "baseUrl": "http://127.0.0.1:8765/v1/openai/openai-direct"
      }
    ],
    "anthropic": [
      {
        "id": "claude-sonnet-4-20250514",
        "name": "Claude Sonnet 4",
        "envKey": "ANTHROPIC_API_KEY",
        "baseUrl": "http://127.0.0.1:8765/v1/anthropic/anthropic-direct"
      }
    ],
    "gemini": [
      {
        "id": "gemini-2.5-pro",
        "name": "Gemini 2.5 Pro",
        "envKey": "GEMINI_API_KEY",
        "baseUrl": "http://127.0.0.1:8765/v1/gemini/gemini-direct"
      }
    ]
  }
}
```

#### 请求路径示意

```
用户选 qwen3-coder-plus:
  POST /v1/openai/dashscope-openai/chat/completions

用户切换到 gpt-4o:
  POST /v1/openai/openai-direct/chat/completions

用户切换到 claude-sonnet-4-20250514:
  POST /v1/anthropic/anthropic-direct/messages

用户切换到 gemini-2.5-pro:
  POST /v1/gemini/gemini-direct/models/gemini-2.5-pro:generateContent
```

这样 Broker 不需要再依赖 API Key 前缀作为主路由依据。

### 5.4 是否支持运行中切换供应商

**支持。**

这正是 Qwen Code 的强项之一：

- `/model` 可以在不同协议和 provider 之间切换
- 每个模型条目有自己的 `baseUrl`
- 每个模型条目有自己的 `envKey`

因此 Qwen Code 的运行中切换能力与 OpenCode 类似，但它的配置更显式，天然适合 Broker 方案。

### 5.5 配置写入与恢复建议

因为 `settings.json` 可能还包含：

- `env`
- `security.auth.selectedType`
- 主题、工具、其他运行配置

所以 `creavor run qwen` 必须采用：

- 合并写入
- 仅修改 `modelProviders.*[].baseUrl`
- 不覆盖其他字段
- 退出时增量恢复
- 提供 `creavor cleanup qwen`

### 5.6 当前实现状态

Qwen Code 的 P0 实现建议为：

- [ ] 读取 `~/.qwen/settings.json` / `.qwen/settings.json`
- [ ] 遍历 `modelProviders`，提取各模型条目的 `baseUrl`
- [ ] 将各模型条目的 `baseUrl` 改写为带 upstream id 的 Broker 地址
- [ ] 保留原有 `envKey`
- [ ] 增量恢复原始 `baseUrl`
- [ ] `creavor cleanup qwen`
- [ ] 不再把 API Key 前缀路由作为主方案

---

## 第六章：Broker 路由增强（更新）

### 6.1 当前路由逻辑（补充说明：详见上文 Path Rewrite & URL Join Rules）

```
统一 Header:
  x-creavor-runtime    → runtime name
  x-creavor-upstream   → upstream id（优先）
  x-creavor-session-id → session correlation（推荐）

路由输入:
  请求路径 → protocol family (anthropic/openai/gemini)
  header x-creavor-runtime → runtime name
  header x-creavor-upstream → upstream id
  session registry / upstream registry → upstream URL
```

### 6.2 Broker 本地注册信息

Broker 维护两类本地状态：

#### A. upstream_registry（静态或半静态）

```json
{
  "upstream_registry": {
    "anthropic-direct": {
      "protocol": "anthropic",
      "upstream": "https://api.anthropic.com"
    },
    "zhipu-anthropic": {
      "protocol": "anthropic",
      "upstream": "https://open.bigmodel.cn/api/anthropic"
    },
    "openai-direct": {
      "protocol": "openai",
      "upstream": "https://api.openai.com/v1"
    },
    "dashscope-openai": {
      "protocol": "openai",
      "upstream": "https://dashscope.aliyuncs.com/compatible-mode/v1"
    },
    "gemini-direct": {
      "protocol": "gemini",
      "upstream": "https://generativelanguage.googleapis.com"
    }
  }
}
```

#### B. session_registry（运行时注册）

```json
{
  "session_registry": {
    "claude-code:a3f2e1:20260409T1430": {
      "runtime": "claude-code",
      "upstream_id": "zhipu-anthropic"
    },
    "codex:b12de9:20260409T1502": {
      "runtime": "codex",
      "upstream_id": "openai-direct"
    }
  }
}
```

### 6.3 路由决策流程

```
Broker 收到请求:

1. 优先读取 x-creavor-upstream
   - 命中 → 直接查 upstream_registry 并路由

2. 若缺失 x-creavor-upstream，则读取 x-creavor-session-id
   - 查 session_registry
   - 命中 → 取出 upstream_id，再查 upstream_registry

3. 若仍未命中，则根据路径推导 protocol family
   - /v1/anthropic/* → anthropic
   - /v1/openai/*    → openai
   - /v1/gemini/*    → gemini

4. 再尝试 runtime 默认上游:
   upstream[runtime]

5. 最后才尝试 key_routing（fallback）
   - 用于无法显式携带 upstream 信息的旧场景
```

### 6.4 API Key 前缀路由的角色（降级为 fallback）

`key_routing` 仍可保留，但仅用于：

- 某些旧配置无法注入 `X-Creavor-Upstream`
- 某些工具暂时不能按模型/provider 写不同 Broker 路径
- 兼容历史版本

**它不再是 OpenCode / Qwen Code 的主方案。**

---

## 第七章：各工具差异对比

| 特性 | Claude Code | OpenCode | Codex | Gemini CLI | Qwen Code |
|------|-------------|----------|-------|------------|-----------|
| 推荐拦截方式 | 环境变量 + custom headers | 配置文件 `provider.*.options.baseURL` | `config.toml` 自定义 provider | `GOOGLE_GEMINI_BASE_URL`（实验） | `settings.json` 中 `modelProviders.*[].baseUrl` |
| 配置文件 | `~/.claude/settings.json` | `~/.config/opencode/opencode.jsonc/json` | `~/.codex/config.toml` | 无稳定主配置路径（P0 以环境变量为主） | `~/.qwen/settings.json` |
| 统一 Header 注入 | `ANTHROPIC_CUSTOM_HEADERS` | `provider.*.options.headers`（静态） | `http_headers` / `env_http_headers` | 不稳定，P0 不强依赖 | 依赖路径表达 upstream，header 非主路径 |
| 动态 Session Header | 支持（启动器/环境变量） | P0 不强依赖 | 支持（`env_http_headers`） | P0 不强依赖 | P0 不强依赖 |
| 多 provider | 否 | 是 | 是（配置层） | 否 / 实验 | 是 |
| 运行中切换 | 否 | 是 | 一般视作启动时确定 | 否 | 是 |
| 主路由依据 | upstream_id + session | 路径或 upstream header | upstream header + config provider | 路径 | 路径（每模型独立 baseUrl） |
| 支持状态 | 正式支持 | 正式支持 | 正式支持 | 实验支持 | 正式支持 |

---

## 附录 A：OpenCode Provider 发现（保留）

### 方式一：通过官方 API 查询运行时 provider

```bash
opencode serve --port 40999 &
sleep 5
curl -s http://127.0.0.1:40999/config/providers
```

### 方式二：读取配置文件中的已启用 provider

优先处理：

- 已在 `provider` 字段中配置的 provider
- `enabled_providers` 中显式启用的 provider
- 已在 `auth.json` 中有凭证的 provider

> P0 不建议“预注入所有 80+ 内置 provider”。

---

## 附录 B：请求流示意图

```
Claude Code:
  claude → ANTHROPIC_BASE_URL=http://127.0.0.1:8765/v1/anthropic
         → ANTHROPIC_CUSTOM_HEADERS=runtime/upstream/session
         → POST /v1/anthropic/messages
         → Broker 查 upstream_registry["zhipu-anthropic"]
         → 转发到上游

OpenCode:
  opencode → provider.zhipuai-coding-plan.options.baseURL=http://127.0.0.1:8765/v1/openai/zhipuai-coding-plan
           → provider.zhipuai-coding-plan.options.headers={runtime,upstream}
           → POST /v1/openai/zhipuai-coding-plan/chat/completions
           → Broker 直接按路径或 x-creavor-upstream 路由

Codex:
  codex → config.toml 中 model_provider=creavor-openai
        → [model_providers.creavor-openai].base_url=http://127.0.0.1:8765/v1/openai/openai-direct
        → http_headers/env_http_headers 注入 runtime/upstream/session
        → POST /v1/openai/openai-direct/responses
        → Broker 查 upstream_registry["openai-direct"]

Gemini CLI:
  gemini → GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:8765/v1/gemini/gemini-direct
         → POST /v1/gemini/gemini-direct/models/gemini-2.5-pro:generateContent
         → Broker 查 upstream_registry["gemini-direct"]
         → 转发到 Google Gemini API
         → 注意：sandbox 模式下需额外 smoke test

Qwen Code:
  qwen → settings.json modelProviders.openai[0].baseUrl=http://127.0.0.1:8765/v1/openai/dashscope-openai
       → settings.json modelProviders.anthropic[0].baseUrl=http://127.0.0.1:8765/v1/anthropic/anthropic-direct
       → settings.json modelProviders.gemini[0].baseUrl=http://127.0.0.1:8765/v1/gemini/gemini-direct
       → /model 切换不同模型时自动走各自的 Broker 路径
       → Broker 按路径直接路由，无需 key prefix 猜测
```
