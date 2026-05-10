<div align="center">

<h1>CLAURST</h1>
<h3><em>Your Favorite Terminal Coding Agent, now in Rust</em></h3>
<img src="public/Rustle.png" alt="Rustle the Crab" width="150" />

<p>
    <a href="https://github.com/kuberwastaken/claurst"><img src="https://img.shields.io/badge/Built_with-Rust-CE4D2B?style=for-the-badge&logo=rust&logoColor=white" alt="Built with Rust"></a>
    <a href="https://github.com/kuberwastaken/claurst"><img src="https://img.shields.io/badge/Version-0.0.9-2E8B57?style=for-the-badge" alt="Version 0.0.9"></a>
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
> - **/goal support:** Try out `/goal <objective>` to see claurst keep working an objective, spanning multiple turns instead of stopping after one normal turn. `[EXPERIMENTAL]`
>
> - **Managed Agents Preview:** Run `/managed-agents` to create a better agentic loop with a Manager-Executor relation and dramatically improved performance for fractions of the cost from running a larger model. Choose from 6 pre-built templates or build your own.`[EXPERIMENTAL]`
>
> - Speech modes: Try `/rocky` and `/caveman` to hear the difference! `/normal` to go back.

---

# Getting Started

## Download a release binary

Grab the latest binary for your platform from [**GitHub Releases**](https://github.com/kuberwastaken/claurst/releases):

| Platform | Binary |
|----------|--------|
| **Windows** x86_64 | `claurst-windows-x86_64.zip` |
| **Linux** x86_64 | `claurst-linux-x86_64.tar.gz` |
| **Linux** aarch64 | `claurst-linux-aarch64.tar.gz` |
| **macOS** Intel | `claurst-macos-x86_64.tar.gz` |
| **macOS** Apple Silicon | `claurst-macos-aarch64.tar.gz` |

### And you're done.

## Build from source

```bash
git clone https://github.com/kuberwastaken/claurst.git
cd claurst/src-rust
cargo build --release --package claurst

# Binary is at target/release/claurst
```

**Raspberry Pi / systems without ALSA** (e.g. Debian Trixie, headless servers):

```bash
# Build without voice/microphone support — no libasound2-dev required
cargo build --release --package claurst --no-default-features
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

Just install and run this from anywhere, that easy.

```bash
# Start Claurst
claurst

# Or run a one-shot headless query
claurst -p "explain this codebase"
```

## Devcontainer setup

After cloning this repository, open it in VS Code and use Reopen in Container to start the development environment.

Prerequisites:
- Docker installed on your host machine: https://www.docker.com/products/docker-desktop/

GPG and SSH forwarding is enabled in the devcontainer, given you have it set up on your host machine. Follow [this guide](https://code.visualstudio.com/remote/advancedcontainers/sharing-git-credentials) if you need help with that.

### Devcontainer features

- Base image: `rust:1-bullseye`.
- Preinstalled build dependencies: `gnupg2`, `libasound2-dev`, `libxdo-dev`, and `pkg-config`.
- Devcontainer features enabled: `common-utils` (with `vscode` user `uid/gid 1000` and Zsh install disabled), `git`, and `docker-outside-of-docker` (`moby: false`).
- Runs as `vscode` user by default.
- Persistent Cargo caches via named volumes for `/usr/local/cargo/registry` and `/usr/local/cargo/git`.
- Binds local `.claurst` into `/home/vscode/.claurst` for local settings/session history access.
- Sets `GNUPGHOME=/home/vscode/.gnupg` and prepends `src-rust/target/debug` and `src-rust/target/release` to `PATH`.
- Post-create setup creates and permissions `.gnupg`, and fixes ownership for `/usr/local/cargo`.
- VS Code setting `terminal.integrated.inheritEnv` is enabled.

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

