# Claurst Installation Guide

Claurst is a Rust reimplementation of the Claude Code CLI. This guide covers
installing from pre-built binaries, verifying the installation, and building
from source.

---

## System Requirements

| Platform | Architecture | Minimum OS |
|----------|-------------|------------|
| Windows  | x86_64      | Windows 10 / Server 2019 |
| Linux    | x86_64      | glibc 2.17+ (most distros from 2014 onward) |
| Linux    | aarch64     | glibc 2.17+ (Raspberry Pi 4, AWS Graviton, etc.) |
| macOS    | x86_64      | macOS 11 Big Sur |
| macOS    | aarch64     | macOS 11 Big Sur (Apple Silicon: M1/M2/M3/M4/M5 and all A-series chips) |

There are no other runtime dependencies. The binary is statically linked where
possible; on Linux it links against the system glibc.

> **Apple Silicon note:** All Apple Silicon Macs — including M1, M2, M3, M4, M5,
> and A-series chips used in MacBook Neo and other devices — use the `aarch64`
> architecture and should use the `claurst-macos-aarch64` binary.

---

## Installing from GitHub Releases

Pre-built binaries are published to the GitHub Releases page:

```
https://github.com/Kuberwastaken/claurst/releases
```

Each release ships five archives:

| Archive | Platform |
|---------|----------|
| `claurst-windows-x86_64.zip` | Windows 64-bit |
| `claurst-linux-x86_64.tar.gz` | Linux x86_64 |
| `claurst-linux-aarch64.tar.gz` | Linux ARM64 |
| `claurst-macos-x86_64.tar.gz` | macOS Intel |
| `claurst-macos-aarch64.tar.gz` | macOS Apple Silicon (M1–M5, A-series) |

### Windows

1. Download `claurst-windows-x86_64.zip` from the latest release.
2. Extract the archive. It contains a single binary: `claurst-windows-x86_64.exe`.
3. Rename it to `claurst.exe` (optional but recommended).
4. Move `claurst.exe` to a directory that is on your `PATH`, for example
   `C:\Users\<you>\bin\` or `C:\Program Files\Claurst\`.
5. Add that directory to your `PATH` if it is not already:
   - Open **Settings > System > About > Advanced system settings > Environment Variables**.
   - Under **User variables**, select `Path` and click **Edit**.
   - Click **New** and enter the directory path.
   - Click **OK** to save.
6. Open a new terminal and verify:

```cmd
claurst --version
```

### Linux (x86_64)

```bash
# Download and extract
curl -L https://github.com/Kuberwastaken/claurst/releases/latest/download/claurst-linux-x86_64.tar.gz \
  | tar -xz

# Make executable and move to a PATH location
chmod +x claurst-linux-x86_64
sudo mv claurst-linux-x86_64 /usr/local/bin/claurst
```

### Linux (aarch64)

```bash
curl -L https://github.com/Kuberwastaken/claurst/releases/latest/download/claurst-linux-aarch64.tar.gz \
  | tar -xz

chmod +x claurst-linux-aarch64
sudo mv claurst-linux-aarch64 /usr/local/bin/claurst
```

### macOS (Intel)

```bash
curl -Lo claurst.tar.gz https://github.com/Kuberwastaken/claurst/releases/latest/download/claurst-macos-x86_64.tar.gz \
  && tar xzf claurst.tar.gz \
  && chmod +x claurst-macos-x86_64 \
  && xattr -rd com.apple.quarantine claurst-macos-x86_64 \
  && sudo mv claurst-macos-x86_64 /usr/local/bin/claurst
```

### macOS (Apple Silicon — M1, M2, M3, M4, M5, A-series)

All Apple Silicon Macs use the same `aarch64` binary regardless of chip
generation (M1 through M5, including A-series chips in MacBook Neo).

```bash
curl -Lo claurst.tar.gz https://github.com/Kuberwastaken/claurst/releases/latest/download/claurst-macos-aarch64.tar.gz \
  && tar xzf claurst.tar.gz \
  && chmod +x claurst-macos-aarch64 \
  && xattr -rd com.apple.quarantine claurst-macos-aarch64 \
  && sudo mv claurst-macos-aarch64 /usr/local/bin/claurst
```

> **macOS Gatekeeper note:** The `xattr -rd com.apple.quarantine` step clears
> the quarantine flag automatically, so macOS will not block the binary on
> first run.

---

## Adding to PATH (general)

If you prefer to install to a user-local directory without `sudo`, move the
binary to `~/.local/bin` instead:

```bash
mkdir -p ~/.local/bin
mv claurst-macos-aarch64 ~/.local/bin/claurst   # adjust filename for your platform
```

Then add `~/.local/bin` to your shell's PATH using the instructions for your
shell below.

### Bash

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

### Zsh

```zsh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

### Fish

Fish uses `fish_add_path` which permanently adds the directory to your PATH
without duplicating it across sessions:

```fish
# Add to PATH for all future sessions (recommended)
fish_add_path ~/.local/bin

# Verify it is on the PATH
echo $PATH
```

`fish_add_path` writes a universal variable that persists across all fish
sessions. You do not need to restart your shell or source any file.

If you prefer to add it via `config.fish` instead:

```fish
# Add to ~/.config/fish/config.fish
echo 'fish_add_path $HOME/.local/bin' >> ~/.config/fish/config.fish
```

---

## Verifying the Installation

```bash
claurst --version
```

A successful installation prints the version string, for example:

```
claude 0.0.9
```

To confirm the binary is the one you installed:

```bash
which claurst          # Linux / macOS / fish
where claurst          # Windows (Command Prompt)
```

---

## Building from Source

Building from source requires the Rust toolchain (stable channel, 1.75 or
later). Install Rust via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

### Option A: Install via Cargo

```bash
cargo install claurst --force
```

This downloads, compiles, and installs the binary to `~/.cargo/bin/claurst`.
That directory is added to `PATH` automatically by `rustup`.

### Option B: Clone and Build

```bash
git clone https://github.com/Kuberwastaken/claurst.git
cd claurst/src-rust

# Debug build (fast to compile, larger binary, extra runtime checks)
cargo build --package claurst

# Release build (optimised, smaller, suitable for everyday use)
cargo build --release --package claurst
```

The release binary is placed at:

```
src-rust/target/release/claurst        # Linux / macOS
src-rust/target\release\claurst.exe   # Windows
```

Copy it to a directory on your `PATH` as described above.

### Linux system dependencies

On Linux, the build requires ALSA development headers (for the optional voice
feature) and OpenSSL:

```bash
# Debian / Ubuntu
sudo apt-get install -y libasound2-dev libssl-dev pkg-config

# Fedora / RHEL
sudo dnf install -y alsa-lib-devel openssl-devel

# Arch
sudo pacman -S alsa-lib openssl
```

### Optional cargo features

| Feature | Description |
|---------|-------------|
| `voice` | Microphone input / voice prompting |
| `computer-use` | Screenshot capture and mouse/keyboard control |
| `dev_full` | All experimental features combined |

To enable a feature:

```bash
cargo build --release --package claurst --features voice
cargo build --release --package claurst --features dev_full
```

### Cross-compiling for Linux aarch64

The release workflow uses [cross](https://github.com/cross-rs/cross) for
aarch64 Linux builds. To reproduce it locally:

```bash
cargo install cross --git https://github.com/cross-rs/cross
cd src-rust
cross build --release --locked --package claurst --target aarch64-unknown-linux-gnu
```

`cross` manages the Docker sysroot, OpenSSL, and ALSA headers automatically.

---

## Shell Completions

Claurst does not currently ship a dedicated `completions` subcommand. All
flags can be discovered via `claurst --help`. If you want basic tab completion
in your shell:

```bash
# bash — add to ~/.bashrc
complete -C claurst claurst

# zsh — add to ~/.zshrc (requires compinit)
compdef _gnu_generic claurst
```

```fish
# fish — add to ~/.config/fish/completions/claurst.fish
complete -c claurst -f -a '(claurst --help 2>&1 | string match -r -- "--\S+")'
```

Richer completion scripts may be added in a future release.

---

## Upgrading

To upgrade to a newer release, repeat the download and replace steps above.
Settings stored in `~/.claurst/` are preserved across upgrades.

To upgrade a source install:

```bash
cargo install claurst --force
```

---

## Uninstalling

Remove the binary:

```bash
sudo rm /usr/local/bin/claurst          # Linux / macOS (system install)
# or
rm ~/.local/bin/claurst                 # user-local install
```

To also remove all settings and session data:

```bash
rm -rf ~/.claurst
```
