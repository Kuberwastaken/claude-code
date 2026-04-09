<div align="center">

<h1>CLAURST</h1>
<h3><em>Your Favorite Terminal Coding Agent, now in Rust</em></h3>
<img src="public/Rustle.png" alt="Rustle the Crab" width="150" />

<p>
    <a href="https://github.com/kuberwastaken/claurst"><img src="https://img.shields.io/badge/Built_with-Rust-CE4D2B?style=for-the-badge&logo=rust&logoColor=white" alt="Built with Rust"></a>
    <a href="https://github.com/kuberwastaken/claurst"><img src="https://img.shields.io/badge/Version-0.0.8-2E8B57?style=for-the-badge" alt="Version 0.0.8"></a>
    <a href="https://github.com/kuberwastaken/claurst/blob/main/LICENSE.md"><img src="https://img.shields.io/badge/License-GPL--3.0-blue?style=for-the-badge" alt="GPL-3.0 License"></a>
</p>

<br />

<img src="public/screenshot.png" alt="CLAURST in action" width="1080" />
</div>

---

Claurst is an **open-source, multi-provider terminal coding agent** built from the ground up in Rust. It started as a clean-room reimplementation of Claude Code's behavior (from [spec](https://github.com/kuberwastaken/claurst/tree/main/spec)) and has since evolved into an amazing TUI pair programmer with multi-provider support, a rich UI, plugin system, a companion named Rustle, chat forking, memory consolidation, and much more.

It's fast, it's memory-efficient, it's yours to run however you want, and there's no tracking or telemetry.

---

> [!NOTE]
> **Recent Updates:**
> - Speech modes: Try /Rocky and /Caveman to hear the difference! /Normal to go back. `[EXPERIMENTAL]`
>
> - Multi-Provider Support is here! Run /Connect to connect to the AI provider of your choice - Anthropic, OpenAI, Google, GitHub Copilot, Ollama, DeepSeek, Groq, Mistral, and [30+ more](#supported-providers). `[EXPERIMENTAL]`

---

## Getting Started

### Download a release binary

Grab the latest binary for your platform from [**GitHub Releases**](https://github.com/kuberwastaken/claurst/releases):

| Platform | Binary |
|----------|--------|
| **Windows** x86_64 | `claurst-windows-x86_64.zip` |
| **Linux** x86_64 | `claurst-linux-x86_64.tar.gz` |
| **Linux** aarch64 | `claurst-linux-aarch64.tar.gz` |
| **macOS** Intel | `claurst-macos-x86_64.tar.gz` |
| **macOS** Apple Silicon | `claurst-macos-aarch64.tar.gz` |

### And you're done.

Just install and run this from anywhere, that easy.

```bash
# Start Claurst
claurst

# Or run a one-shot headless query
claurst -p "explain this codebase"
```

## Build from source

```bash
git clone https://github.com/kuberwastaken/claurst.git
cd claurst/src-rust
cargo build --release --package claurst

# Binary is at target/release/claurst
# Run with:
cargo run -p claurst
```

### First run

```bash
# Set your API key (or use /connect inside Claurst to configure)
export ANTHROPIC_API_KEY=sk-ant-...

# Start Claurst
claurst

# Or run a one-shot headless query
claurst -p "explain this codebase"
```

## Documentation

For more info on how to configure Claurst, [head over to our docs](https://claurst.kuber.studio/docs).

>**PS:** The original breakdown of the findings from Claude Code's source that started this project is on [my blog](https://kuber.studio/blog/AI/Claude-Code's-Entire-Source-Code-Got-Leaked-via-a-Sourcemap-in-npm,-Let's-Talk-About-it) - the full technical writeup of what was found, how the leak happened, and what it revealed.

---

## Contributing

Claurst is built for the community, by the community and we'd love your help making it better.

[Open an issue](https://github.com/Kuberwastaken/claurst/issues/new) for bugs, ideas, or questions, or [Raise a PR](https://github.com/Kuberwastaken/claurst/pulls/new) to fix bugs, add features, or improve documentation.

---

## Important Notice

This repository does not hold a copy of the proprietary Claude Code TypeScript source code.
This is a **clean-room Rust reimplementation** of Claude Code's behavior.

The process was explicitly two-phase:

**Specification** [`spec/`](https://github.com/kuberwastaken/claurst/tree/main/spec) — An AI agent analyzed the source and produced exhaustive behavioral specifications and improvements, deviated from the original: architecture, data flows, tool contracts, system designs. No source code was carried forward.

**Implementation** [`src-rust/`](https://github.com/kuberwastaken/claurst/tree/main/src-rust) — A separate AI agent implemented from the spec alone, never referencing the original TypeScript. The output is idiomatic Rust that reproduces the behavior, not the expression.

This mirrors the legal precedent established by Phoenix Technologies v. IBM (1984) — clean-room engineering of the BIOS — and the principle from Baker v. Selden (1879) that copyright protects expression, not ideas or behavior.

---

