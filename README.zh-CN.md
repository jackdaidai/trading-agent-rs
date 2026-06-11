# trading-agent-rs

[English](README.md) | **简体中文**

trading-agent-rs 是一个 Rust 原生的 AI 股票分析 Agent。它可以把一个或多个股票代码转换成 Markdown 研究报告，报告会综合市场数据、基本面、新闻、多 Agent 辩论和 LLM 综合分析。

本项目受 [TauricResearch/TradingAgents](https://github.com/TauricResearch/TradingAgents) 启发。它不是逐行移植，而是在保留 TradingAgents 风格的分析师、辩论、交易员、风险评审和投资组合经理流程的同时，优化为原生执行、更低编排开销、可控批处理并发和单二进制文件运行。

> 本项目仅用于研究和教育目的，不构成金融建议、投资建议，也不构成买入、卖出或持有任何证券的建议。
>
> trading-agent-rs 受 [TauricResearch/TradingAgents](https://github.com/TauricResearch/TradingAgents) 启发，后者采用 Apache-2.0 许可证。

## 为什么选择 trading-agent-rs

- **Rust-first 运行时：** 常规 CLI 使用不需要 Python 运行时。
- **原生数据获取：** 通过 Rust HTTP 请求访问 Yahoo Finance 公共端点，获取行情、技术指标、基本面和新闻。
- **TradingAgents 风格推理：** 分析师报告进入多空研究、交易计划、风险辩论和投资组合经理综合流程。
- **适合批处理：** 股票代码校验和多股票分析使用 async Rust，并支持可配置并发。
- **Provider 灵活：** MiniMax、Z.ai、OpenAI-compatible 和 Anthropic-compatible provider 通过统一 LLM client 抽象接入。
- **文件化输出：** 默认将生成报告保存到 `reports/`，便于查看和归档。

## 分析流程

<p align="center">
  <img src="assets/tagent-pipeline.svg" alt="trading-agent-rs 多 Agent 交易分析流程" style="width: 100%; height: auto;">
</p>

trading-agent-rs 使用固定研究流程：

1. 市场、情绪、新闻和基本面分析师通过 Yahoo Finance 工具收集证据。
2. 多头和空头研究员辩论投资逻辑。
3. 研究经理把辩论结果综合成投资计划。
4. 交易员把计划转换成具体交易逻辑。
5. 风险分析师从激进、安全和中性视角展开风险辩论。
6. 投资组合经理写出最终决策和报告。

## 快速开始

### 前置条件

- Rust 1.80 或更新版本，以及 Cargo
- 一个受支持 LLM provider 的 API key

### 设置

```powershell
git clone <repo-url>
cd trading-agent-rs

Copy-Item .env.example .env
```

编辑 `.env`，设置要使用的 provider 和 API key。默认 provider 是 MiniMax：

```dotenv
TRADING_AGENT_PROVIDER=minimax
MINIMAX_API_KEY=your-key-here
```

构建并运行本地检查：

```powershell
cargo test
cargo build --release
```

Release 构建后的 Windows 二进制文件位于 `target\release\trading-agent-rs.exe`。

## CLI 用法

不需要 API key 即可查看帮助：

```powershell
cargo run -- --help
```

只校验配置，不运行市场分析：

```powershell
cargo run -- config check
```

列出支持的 provider：

```powershell
cargo run -- providers
```

使用本地今天日期分析单只股票：

```powershell
cargo run -- AAPL
```

使用指定日期分析单只股票：

```powershell
cargo run -- AAPL --date 2026-04-30
```

使用受控批处理并发分析多只股票：

```powershell
cargo run -- analyze AAPL MSFT NVDA --date 2026-04-30 --concurrency 2
```

仍然支持旧的位置参数日期语法：

```powershell
cargo run -- AAPL MSFT NVDA 2026-04-30
```

对过往决策按实际收益打分，并把经验教训写入 Agent 记忆（未到期的决策保持 pending）：

```powershell
cargo run -- resolve --horizon-days 14
```

生成的报告会写入 `reports\<TICKER>_<DATE>.md`，除非通过 `--reports-dir` 或 `TRADING_AGENT_REPORTS_DIR` 指定其他目录。

## 配置

trading-agent-rs 会自动加载 `.env`。通用 `TRADING_AGENT_*` 变量会覆盖 provider 专用变量，CLI 参数会覆盖当前运行的环境变量。为了兼容旧版本，仍然支持 `TAGENT_*` 变量。

| 变量 | CLI 参数 | 说明 | 默认值 |
| --- | --- | --- | --- |
| `TRADING_AGENT_PROVIDER` | `--provider` | `minimax`、`zai`、`openai` 或 `anthropic` | `minimax` |
| `TRADING_AGENT_API_KEY` | 未设置 | 通用 API key 覆盖项 | 未设置 |
| `TRADING_AGENT_BASE_URL` | 未设置 | 通用 Base URL 覆盖项 | provider 默认值 |
| `TRADING_AGENT_MODEL` | `--model` | quick 和 deep 调用共用的模型覆盖项 | provider 默认值 |
| `TRADING_AGENT_QUICK_MODEL` | `--quick-model` | 分析师和工具密集型调用使用的模型 | `TRADING_AGENT_MODEL` |
| `TRADING_AGENT_DEEP_MODEL` | `--deep-model` | 综合分析调用使用的模型 | `TRADING_AGENT_MODEL` |
| `TRADING_AGENT_BATCH_CONCURRENCY` | `--concurrency` | 批处理模式下并发分析的股票数量 | `1` |
| `TRADING_AGENT_LLM_CONCURRENCY` | 未设置 | 进程级 LLM 并发请求上限（rate limit 保护） | `4` |
| `TRADING_AGENT_LLM_TIMEOUT` | 未设置 | LLM 请求超时（秒） | `240` |
| `TRADING_AGENT_MAX_TOKENS` | 未设置 | Anthropic 格式响应的最大输出 token 数；被截断时会记录警告 | `4096` |
| `TRADING_AGENT_QUICK_THINKING` | 未设置 | 设为 `off` 可关闭 quick 模型（辩论/风险）调用的扩展思考 | `on` |
| `TRADING_AGENT_REPORTS_DIR` | `--reports-dir` | 生成 Markdown 报告的目录 | `reports` |
| `TRADING_AGENT_YAHOO_BASE_URL` | 未设置 | 可选 Yahoo Finance Base URL 覆盖项 | `https://query1.finance.yahoo.com` |

Provider 专用变量：

| Provider | API key | Base URL | 默认模型 |
| --- | --- | --- | --- |
| MiniMax | `MINIMAX_API_KEY` | `MINIMAX_BASE_URL` | `MiniMax-M2.7` |
| Z.ai | `ZAI_API_KEY` | `ZAI_BASE_URL` | `GLM-5.1` |
| OpenAI | `OPENAI_API_KEY` | `OPENAI_BASE_URL` | `gpt-4o` |
| Anthropic | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL` | `claude-sonnet-4-6` |

无论批处理并发设多大，`TRADING_AGENT_LLM_CONCURRENCY` 都会限制同时在途的 LLM 请求数，因此对大多数 provider 来说 `--concurrency 4` 是安全的。如果 provider 仍然返回 429，把 `TRADING_AGENT_LLM_CONCURRENCY` 降到 2-3。

## 输出示例

生成报告是包含最终投资组合经理决策的 Markdown 文件。具体措辞取决于市场数据和所选模型，但结构类似：

```markdown
# AAPL Analysis - 2026-04-30

> For research and education only. Not financial advice, investment advice, or a recommendation to buy, sell, or hold any security.

**Completed in 84s**

## Final Decision

Action: HOLD
Confidence: Moderate

Rationale:
- Technical momentum is mixed while recent news does not justify an aggressive entry.
- Fundamentals remain resilient, but valuation leaves limited margin of safety.
- Risk review favors waiting for a clearer catalyst or improved entry price.

Risk Controls:
- Re-evaluate if price breaks key support or volume confirms a trend reversal.
- Avoid oversized exposure while macro and earnings uncertainty remain elevated.
```

更长的模拟报告见 [examples/mock-report.md](examples/mock-report.md)。

## 当前限制和路线图

trading-agent-rs 有意从比上游 Python 项目更小的范围开始。当前重点是快速、原生的 CLI 工作流。未来工作包括：

| 领域 | 当前状态 | 目标方向 |
| --- | --- | --- |
| 交互式 CLI | 非交互式 CLI，提供 `clap` 帮助和子命令 | 如果需求明确，增加更丰富的引导式提示 |
| Docker | 尚未打包 | 添加 `Dockerfile` 和 compose 示例 |
| 本地模型 | 尚未接入 Ollama | 添加 Ollama/OpenAI-compatible 本地配置 |
| Provider 覆盖 | MiniMax、Z.ai、OpenAI-compatible、Anthropic-compatible | 考虑 Gemini、DeepSeek、Qwen、OpenRouter、Azure 和 Bedrock |
| 持久化 | 决策日志和 BM25 Agent 记忆持久化在 `~/.trading-agent-rs/` 下；`resolve` 子命令对过往决策打分并记录经验 | 对已结算决策做更丰富的结果分析 |
| 断点续跑 | 尚未实现 | 为中断的长任务添加可恢复图执行 |
| Benchmark | 已记录性能目标，但未发布 benchmark 结果 | 添加可复现的多股票 benchmark 脚本和结果 |

## 开发

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

GitHub Actions CI 会在每次 push 和 pull request 上运行这些检查。

Yahoo Finance 数据通过 Rust 原生获取；常规使用不需要 Python 运行时。Yahoo Finance 公共端点可能变化或被 rate limit，因此实时数据测试应视为 smoke test，而不是确定性的单元测试。

## 项目文档

- [贡献指南](CONTRIBUTING.md)
- [架构说明](docs/ARCHITECTURE.md)
- [开发指南](docs/DEVELOPMENT.md)
- [安全策略](SECURITY.md)
- [发布指南](docs/RELEASE.md)
- [模拟报告示例](examples/mock-report.md)

## 许可证和归属

trading-agent-rs 使用 Apache License, Version 2.0。见 [LICENSE](LICENSE)。

本项目是受 [TauricResearch/TradingAgents](https://github.com/TauricResearch/TradingAgents) 启发的 Rust 重构版本，后者同样使用 Apache-2.0。见 [NOTICE](NOTICE)。
