#!/usr/bin/env bash
# Claurst installer for Linux and macOS.
#
# Usage (one-liner):
#   curl -fsSL https://github.com/Kuberwastaken/claurst/releases/latest/download/install.sh | bash
#
# Or download and run locally:
#   curl -fsSL -O https://github.com/Kuberwastaken/claurst/releases/latest/download/install.sh
#   chmod +x install.sh
#   ./install.sh

set -euo pipefail

APP=claurst
REPO=Kuberwastaken/claurst

# ANSI colours
MUTED='\033[0;2m'
RED='\033[0;31m'
GREEN='\033[0;32m'
ORANGE='\033[38;5;214m'
NC='\033[0m'

usage() {
    cat <<EOF
Claurst installer

Usage: install.sh [options]

Options:
    -h, --help              Display this help message
    -v, --version <version> Install a specific version (e.g., 0.0.9 or v0.0.9)
    -b, --binary <path>     Install from a local binary instead of downloading
        --no-modify-path    Don't modify shell config files (.zshrc, .bashrc, etc.)
        --install-dir <dir> Override install location (default: ~/.claurst/bin)

Examples:
    curl -fsSL https://github.com/Kuberwastaken/claurst/releases/latest/download/install.sh | bash
    ./install.sh --version 0.0.9
    ./install.sh --binary /path/to/claurst
EOF
}

print_message() {
    local level=$1
    local message=$2
    local color=""
    case "$level" in
        info)    color="$NC" ;;
        success) color="$GREEN" ;;
        warning) color="$ORANGE" ;;
        error)   color="$RED" ;;
        *)       color="$NC" ;;
    esac
    printf "${color}%b${NC}\n" "$message"
}

requested_version=${VERSION:-}
no_modify_path=false
binary_path=""
install_dir_override=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        -v|--version)
            if [[ -n "${2:-}" ]]; then
                requested_version="$2"
                shift 2
            else
                print_message error "Error: --version requires a version argument"
                exit 1
            fi
            ;;
        -b|--binary)
            if [[ -n "${2:-}" ]]; then
                binary_path="$2"
                shift 2
            else
                print_message error "Error: --binary requires a path argument"
                exit 1
            fi
            ;;
        --no-modify-path)
            no_modify_path=true
            shift
            ;;
        --install-dir)
            if [[ -n "${2:-}" ]]; then
                install_dir_override="$2"
                shift 2
            else
                print_message error "Error: --install-dir requires a path argument"
                exit 1
            fi
            ;;
        *)
            print_message warning "Warning: Unknown option '$1'"
            shift
            ;;
    esac
done

INSTALL_DIR="${install_dir_override:-$HOME/.claurst/bin}"
mkdir -p "$INSTALL_DIR"

# ----- Detect platform & arch -----
detect_target() {
    local raw_os arch
    raw_os=$(uname -s)
    case "$raw_os" in
        Darwin*)              os="macos" ;;
        Linux*)               os="linux" ;;
        MINGW*|MSYS*|CYGWIN*)
            print_message error "Detected Windows-like environment ($raw_os)."
            print_message info "Run install.ps1 in PowerShell instead:"
            print_message info "  irm https://github.com/${REPO}/releases/latest/download/install.ps1 | iex"
            exit 1
            ;;
        *)
            print_message error "Unsupported OS: $raw_os"
            exit 1
            ;;
    esac

    arch=$(uname -m)
    case "$arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)
            print_message error "Unsupported architecture: $arch"
            exit 1
            ;;
    esac

    # Apple Silicon under Rosetta: prefer the native arm64 binary.
    if [ "$os" = "macos" ] && [ "$arch" = "x86_64" ]; then
        rosetta_flag=$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)
        if [ "$rosetta_flag" = "1" ]; then
            arch="aarch64"
        fi
    fi

    target="${os}-${arch}"
}

# ----- Pre-flight: required tools -----
check_required_tools() {
    local missing=()
    command -v curl >/dev/null 2>&1 || missing+=("curl")
    command -v tar  >/dev/null 2>&1 || missing+=("tar")
    if [ "${#missing[@]}" -gt 0 ]; then
        print_message error "Missing required tools: ${missing[*]}"
        print_message info "Please install them and try again."
        exit 1
    fi
}

resolve_version() {
    if [[ -n "$requested_version" ]]; then
        # Strip leading 'v' if present
        requested_version="${requested_version#v}"
        specific_version="$requested_version"
    else
        # Fetch latest version from GitHub API.  Use sed instead of jq for portability.
        specific_version=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' \
            | head -n 1)
        if [[ -z "$specific_version" ]]; then
            print_message error "Failed to fetch latest version from GitHub API"
            exit 1
        fi
    fi
}

# ----- Already-installed check -----
check_existing_install() {
    if command -v "$APP" >/dev/null 2>&1; then
        existing_path=$(command -v "$APP")
        installed_version=$("$APP" --version 2>/dev/null | awk '{print $NF}' || echo "unknown")
        if [[ "$installed_version" == "$specific_version" ]]; then
            print_message info "${MUTED}Version ${NC}$specific_version${MUTED} already installed at ${NC}$existing_path"
            print_message info "${MUTED}Use --version to install a different version, or pass a different one to upgrade.${NC}"
            exit 0
        fi
        print_message info "${MUTED}Found existing claurst at ${NC}$existing_path${MUTED} (v$installed_version) - upgrading to v$specific_version${NC}"
    fi
}

# ----- Download & extract -----
download_and_install() {
    local archive="${APP}-${target}.tar.gz"
    local url="https://github.com/${REPO}/releases/download/v${specific_version}/${archive}"
    local tmp_dir
    tmp_dir=$(mktemp -d -t claurst-install-XXXXXX)
    trap "rm -rf '$tmp_dir'" EXIT

    print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}v${NC}${specific_version} ${MUTED}(${target})${NC}"
    print_message info "${MUTED}Downloading ${NC}${url}"

    if ! curl -fL --progress-bar -o "$tmp_dir/$archive" "$url"; then
        print_message error "Download failed."
        print_message info "Check that release v${specific_version} exists for ${target}:"
        print_message info "  https://github.com/${REPO}/releases/tag/v${specific_version}"
        exit 1
    fi

    print_message info "${MUTED}Extracting...${NC}"
    tar -xzf "$tmp_dir/$archive" -C "$tmp_dir"

    if [[ ! -f "$tmp_dir/$APP" ]]; then
        print_message error "Archive did not contain expected binary '$APP'"
        print_message info "Contents:"
        ls -la "$tmp_dir"
        exit 1
    fi

    install_binary "$tmp_dir/$APP"
}

install_from_binary() {
    if [[ ! -f "$binary_path" ]]; then
        print_message error "Binary not found at $binary_path"
        exit 1
    fi
    print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}from ${NC}$binary_path"
    install_binary "$binary_path"
}

install_binary() {
    local source="$1"
    local target_path="${INSTALL_DIR}/${APP}"

    cp "$source" "$target_path"
    chmod 755 "$target_path"

    # On macOS, strip the quarantine attribute so Gatekeeper doesn't block the unsigned binary.
    if [[ "$(uname -s)" == "Darwin" ]]; then
        xattr -dr com.apple.quarantine "$target_path" 2>/dev/null || true
    fi

    print_message success "Installed: $target_path"
}

# ----- PATH modification -----
add_to_path_if_needed() {
    if [[ "$no_modify_path" == "true" ]]; then
        return
    fi

    if [[ ":$PATH:" == *":$INSTALL_DIR:"* ]]; then
        return  # already on PATH for this session
    fi

    XDG_CONFIG_HOME=${XDG_CONFIG_HOME:-$HOME/.config}
    local current_shell config_files=""
    current_shell=$(basename "${SHELL:-/bin/bash}")

    case "$current_shell" in
        fish)
            config_files="$HOME/.config/fish/config.fish"
            ;;
        zsh)
            config_files="${ZDOTDIR:-$HOME}/.zshrc ${ZDOTDIR:-$HOME}/.zshenv $XDG_CONFIG_HOME/zsh/.zshrc"
            ;;
        bash)
            config_files="$HOME/.bashrc $HOME/.bash_profile $HOME/.profile"
            ;;
        ash|sh)
            config_files="$HOME/.profile /etc/profile"
            ;;
        *)
            config_files="$HOME/.bashrc $HOME/.bash_profile $HOME/.profile"
            ;;
    esac

    local config_file=""
    for file in $config_files; do
        if [[ -f "$file" ]]; then
            config_file="$file"
            break
        fi
    done

    # Fall back to creating ~/.profile if nothing exists.
    if [[ -z "$config_file" ]]; then
        config_file="$HOME/.profile"
        touch "$config_file"
    fi

    local path_line
    if [[ "$current_shell" == "fish" ]]; then
        path_line="fish_add_path $INSTALL_DIR"
    else
        path_line="export PATH=\"$INSTALL_DIR:\$PATH\""
    fi

    if grep -Fq "$INSTALL_DIR" "$config_file" 2>/dev/null; then
        print_message info "${MUTED}PATH entry already in ${NC}$config_file"
        return
    fi

    {
        echo ""
        echo "# claurst"
        echo "$path_line"
    } >> "$config_file"
    print_message success "Added $INSTALL_DIR to PATH in $config_file"
    print_message info "${MUTED}Restart your shell or run:${NC} source $config_file"
}

# ----- GitHub Actions environment hint -----
github_path_hint() {
    if [ -n "${GITHUB_ACTIONS-}" ] && [ "${GITHUB_ACTIONS}" = "true" ] && [ -n "${GITHUB_PATH-}" ]; then
        echo "$INSTALL_DIR" >> "$GITHUB_PATH"
        print_message info "Added $INSTALL_DIR to \$GITHUB_PATH"
    fi
}

# ----- Main flow -----
main() {
    check_required_tools

    if [[ -n "$binary_path" ]]; then
        # Local binary install - skip detection & version resolution.
        specific_version="local"
        install_from_binary
    else
        detect_target
        resolve_version
        check_existing_install
        download_and_install
    fi

    add_to_path_if_needed
    github_path_hint

    # Goodbye banner
    echo ""
    print_message success "claurst is installed!"
    echo ""
    echo -e "${MUTED}Quickstart:${NC}"
    echo -e "  ${MUTED}# Set an API key${NC}"
    echo -e "  export ANTHROPIC_API_KEY=sk-ant-..."
    echo -e ""
    echo -e "  ${MUTED}# Open a new terminal, then:${NC}"
    echo -e "  ${GREEN}claurst${NC}              ${MUTED}# Interactive TUI${NC}"
    echo -e "  ${GREEN}claurst -p \"...\"${NC}       ${MUTED}# Headless one-shot${NC}"
    echo ""
    echo -e "${MUTED}Docs: ${NC}https://github.com/${REPO}"
}

main "$@"
