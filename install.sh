#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Installs the release binary for this system once its checksum and bench match the release's, or
# builds Cadence from source at the release's tag where no binary fits or the check fails, on Linux,
# macOS, and Windows under Git Bash or WSL. It asks before building, installing a compiler or Rust,
# replacing an install, changing the PATH or downloading tablebases, unless --yes is given.

set -u

REPO_URL="https://github.com/grounzero/cadence.git"
ISSUES_URL="https://github.com/grounzero/cadence/issues"
API_URL="https://api.github.com/repos/grounzero/cadence/releases"
# Each release's files, by version; a test points it at a folder it serves itself.
ASSETS_URL="${CADENCE_RELEASES_URL:-https://github.com/grounzero/cadence/releases/download}"
TABLES_URL="https://tablebase.lichess.ovh/tables/standard"
# The 290 tables of five pieces or fewer, in bytes, so their progress has a total before any arrives.
TABLES_BYTES=983957920
MARK="# Added by the Cadence installer"
# The workspace version these scripts ship with, which CI holds to Cargo.toml, so a version cut
# changes it here too.
INSTALLER_VERSION="0.6.0-rc.3"

YES=0
KEEP_LOG=0
MODE=install
VERSION=""
PREFIX=""
BUILD=""
SOURCE=0
STOPPED=0
LOG=""
OUT=""
WHY=""
BUILT=""

usage() {
    cat <<'EOF'
Usage: install.sh [options]

Installs Cadence's release binary for this system once its checksum and bench match the
release's, or builds it from source where no binary fits. It asks before building, installing a
compiler or Rust, replacing an install, changing your PATH or downloading tablebases.

  --version V      this release rather than the latest; with --source, also a test base x.y.z+n
  --dir DIR        install into DIR rather than the default folder; --prefix DIR is the same
  --generic        on x86-64, the generic build, for any processor with POPCNT
  --build TIER     on x86-64, the avx2 or the generic build; avx2 only where the processor runs it
  --source         build from source even where a release binary fits
  --update         install the latest release over an installed Cadence
  --uninstall      remove Cadence, and the PATH entry and tablebases if this script added them
  --yes            take every default without asking
  --keep-log       keep the log even when everything succeeds
  --help           show this and stop
EOF
}

# First on every run and first in the log, so any output says which installer wrote it.
echo "Cadence installer $INSTALLER_VERSION"

while [ $# -gt 0 ]; do
    case "$1" in
        --yes | -y) YES=1 ;;
        --keep-log) KEEP_LOG=1 ;;
        --update) MODE=update ;;
        --uninstall) MODE=uninstall ;;
        --source) SOURCE=1 ;;
        --generic) BUILD=generic ;;
        --version | --dir | --prefix | --build)
            [ $# -ge 2 ] || { echo "$1 needs a value" >&2; exit 2; }
            case "$1" in
                --version) VERSION=$2 ;;
                --dir | --prefix) PREFIX=$2 ;;
                --build) BUILD=$2 ;;
            esac
            shift
            ;;
        --help | -h) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done
case "$BUILD" in
    "" | avx2 | generic) ;;
    *) echo "--build takes avx2 or generic" >&2; exit 2 ;;
esac

# --- The system --------------------------------------------------------------------------------

OS=""
WSL=0
EXE=""
case "$(uname -s)" in
    Linux)
        OS=linux
        if grep -qi microsoft /proc/version 2>/dev/null; then WSL=1; fi
        ;;
    Darwin) OS=macos ;;
    MINGW* | MSYS* | CYGWIN*) OS=windows; EXE=.exe ;;
    *)
        echo "This system, $(uname -s), is not one this script knows; the README says how to build by hand." >&2
        exit 1
        ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) ARCH=x86_64 ;;
    aarch64 | arm64) ARCH=aarch64 ;;
    *) echo "This processor, $(uname -m), is not one Cadence is built for." >&2; exit 1 ;;
esac
# A shell under Rosetta reports x86_64 on Apple silicon, which runs the arm64 build natively.
if [ "$OS" = macos ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ]; then ARCH=aarch64; fi

if [ "$OS" = windows ]; then
    [ -n "${LOCALAPPDATA:-}" ] || { echo "LOCALAPPDATA is not set, so give --dir." >&2; exit 1; }
    APP_DIR="$(cygpath -u "$LOCALAPPDATA")/Cadence"
    DEFAULT_PREFIX=$APP_DIR
    DATA_DIR=$APP_DIR
    LOG_DIR=$APP_DIR
    CARGO_BIN="$(cygpath -u "${USERPROFILE:-$HOME}")/.cargo/bin"
else
    DEFAULT_PREFIX="$HOME/.local/bin"
    DATA_DIR="$HOME/.local/share/cadence"
    LOG_DIR="$HOME/.cache/cadence"
    CARGO_BIN="$HOME/.cargo/bin"
fi
STATE="$DATA_DIR/install-state"
WORK="$DATA_DIR/build.$$"

# --- The log -----------------------------------------------------------------------------------

mkdir -p "$LOG_DIR" || { echo "Cannot create $LOG_DIR for the log." >&2; exit 1; }
LOG="$LOG_DIR/install-$(date +%Y%m%d-%H%M%S).log"
OUT="$LOG.out"
: > "$LOG" || { echo "Cannot write the log at $LOG." >&2; exit 1; }

# The home folder is written as ~ in every spelling the platform uses, so the log names no user.
HOME_UNIX=""
HOME_WIN=""
HOME_MIXED=""
[ "${HOME:-/}" = / ] || HOME_UNIX=$HOME
if [ "$OS" = windows ] && [ -n "$HOME_UNIX" ]; then
    HOME_WIN=$(cygpath -w "$HOME_UNIX")
    HOME_MIXED=$(cygpath -m "$HOME_UNIX")
fi
redact() {
    CADENCE_H1=$HOME_WIN CADENCE_H2=$HOME_MIXED CADENCE_H3=$HOME_UNIX awk '
        function swap(s, h,    i) {
            if (h == "") return s
            while ((i = index(s, h)) > 0) s = substr(s, 1, i - 1) "~" substr(s, i + length(h))
            return s
        }
        {
            s = swap($0, ENVIRON["CADENCE_H1"])
            s = swap(s, ENVIRON["CADENCE_H2"])
            print swap(s, ENVIRON["CADENCE_H3"])
        }'
}

log() { printf '%s\n' "$*" | redact >> "$LOG"; }
say() { printf '%s\n' "$*"; log "$*"; }
step() { say ""; say "== $*"; }
fail() { say "Error: $*"; exit 1; }

# Shown as it runs and written to the log with its exit status.
run() {
    log "\$ $*"
    "$@" 2>&1 | tee "$OUT"
    local rc=${PIPESTATUS[0]}
    redact < "$OUT" >> "$LOG"
    log "(exit $rc)"
    return "$rc"
}

finish() {
    local rc=$?
    rm -f "$OUT"
    rm -rf "$WORK"
    if [ "$rc" -eq 0 ] || [ "$STOPPED" = 1 ]; then
        if [ "$KEEP_LOG" = 1 ]; then echo "The log is at $LOG."; else rm -f "$LOG"; fi
    else
        echo
        echo "The log is at $LOG."
        echo "Attach this file to a GitHub issue: $ISSUES_URL"
    fi
}
trap finish EXIT

stop() {
    say "$*"
    STOPPED=1
    exit 1
}

# Yes or no, the default in capitals; --yes takes the default.
ask() {
    local question=$1 default=$2 hint reply
    if [ "$default" = y ]; then hint="[Y/n]"; else hint="[y/N]"; fi
    if [ "$YES" = 1 ]; then
        say "$question $hint: $default (--yes)"
        [ "$default" = y ]
        return
    fi
    while true; do
        printf '%s %s ' "$question" "$hint"
        IFS= read -r reply < /dev/tty || fail "there is no terminal to answer on; run with --yes to take every default"
        log "$question $hint: ${reply:-<enter>}"
        case "$reply" in
            "") [ "$default" = y ]; return ;;
            y | Y | yes | Yes | YES) return 0 ;;
            n | N | no | No | NO) return 1 ;;
            *) echo "Please answer y or n." ;;
        esac
    done
}

# A numbered choice whose answer is left in CHOICE; --yes takes the first.
choose() {
    local question=$1 i=1 reply option
    shift
    say "$question"
    for option in "$@"; do
        say "  $i. $option"
        i=$((i + 1))
    done
    if [ "$YES" = 1 ]; then
        CHOICE=1
        say "Choice: 1 (--yes)"
        return
    fi
    while true; do
        printf 'Choice [1]: '
        IFS= read -r reply < /dev/tty || fail "there is no terminal to answer on; run with --yes to take every default"
        log "Choice: ${reply:-<enter>}"
        case "${reply:-1}" in
            [1-9]) if [ "${reply:-1}" -lt "$i" ]; then CHOICE=${reply:-1}; return; fi ;;
        esac
        echo "Please answer with one of the numbers."
    done
}

state_get() {
    [ -f "$STATE" ] || return 0
    sed -n "s/^$1=//p" "$STATE" | head -n 1
}

state_set() {
    mkdir -p "$DATA_DIR" || fail "cannot create $DATA_DIR"
    { [ -f "$STATE" ] && grep -v "^$1=" "$STATE"; printf '%s=%s\n' "$1" "$2"; } > "$STATE.new" \
        && mv -f "$STATE.new" "$STATE" || fail "cannot write $STATE"
}

shown() {
    if [ "$OS" = windows ]; then cygpath -w "$1"; else printf '%s' "$1"; fi
}

checksum() {
    if command -v sha256sum > /dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi \
        | awk '{print $1}'
}

download() {
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$2" "$1"
    elif command -v wget > /dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        fail "neither curl nor wget is installed, so nothing can be downloaded"
    fi
}

# The last line a failed download wrote, in parentheses, or nothing.
said() {
    local last
    last=$(tail -n 1 "$OUT" 2>/dev/null | tr -d '\r')
    [ -z "$last" ] || printf ' (%s)' "$last"
}

system_report() {
    log "Cadence installer $INSTALLER_VERSION"
    log "installer: $MODE${VERSION:+ $VERSION}"
    log "system: $(uname -srm)"
    case "$OS" in
        linux)
            [ ! -r /etc/os-release ] || log "os: $(sed -n 's/^PRETTY_NAME=//p' /etc/os-release | tr -d '"')"
            [ "$WSL" = 0 ] || log "os: under WSL"
            ;;
        macos) log "os: macOS $(sw_vers -productVersion 2>/dev/null)" ;;
        windows) log "os: $(cmd //c ver 2>/dev/null | tr -d '\r' | sed '/^$/d')" ;;
    esac
    local cpu=""
    if [ "$OS" = macos ]; then
        cpu=$(sysctl -n machdep.cpu.brand_string 2>/dev/null)
    elif [ -r /proc/cpuinfo ]; then
        cpu=$(sed -n 's/^model name[[:space:]]*: //p' /proc/cpuinfo | head -n 1)
    fi
    if [ -z "$cpu" ] && command -v lscpu > /dev/null 2>&1; then
        cpu=$(lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -n 1)
    fi
    log "cpu: ${cpu:-unknown}, $(uname -m)"
}

# A release's x.y.z or x.y.z-rc.n, or with --source a test base's x.y.z+n, which has no release.
check_version() {
    [ -n "$VERSION" ] || return 0
    if printf '%s' "$VERSION" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+\+[0-9]+'; then
        [ "$SOURCE" = 1 ] || fail "$VERSION is a test base and not a release, so it has no binary; --source builds it"
        return 0
    fi
    printf '%s' "$VERSION" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?' \
        || fail "$VERSION is not a release's version, which reads x.y.z or x.y.z-rc.n"
}

describe_system() {
    local os=Linux arch=x86-64
    case "$OS" in
        macos) os=macOS ;;
        windows) os=Windows ;;
    esac
    [ "$WSL" = 0 ] || os="Linux under WSL"
    if [ "$ARCH" = aarch64 ]; then
        if [ "$OS" = linux ]; then arch=aarch64; else arch=arm64; fi
    fi
    printf '%s on %s' "$os" "$arch"
}

# The x86-64 build this processor runs, from the features each needs: avx2 (AVX2, BMI1, LZCNT and
# POPCNT), generic (POPCNT) or none, with LACKS naming what is missing; unread where none can be read.
x86_level() {
    local flags="" f
    LACKS=""
    if [ "$OS" = macos ]; then
        flags=$({ sysctl -n machdep.cpu.features; sysctl -n machdep.cpu.leaf7_features
            sysctl -n machdep.cpu.extfeatures; } 2>/dev/null | tr '[:upper:]' '[:lower:]' | tr '\n' ' ')
    elif [ -r /proc/cpuinfo ]; then
        flags=$(sed -n 's/^flags[[:space:]]*: //p' /proc/cpuinfo | head -n 1)
    fi
    if [ -z "$flags" ]; then
        LEVEL=unread
        return
    fi
    # /proc/cpuinfo names LZCNT abm, and on Windows prints abm only on AMD. Every processor with AVX2
    # has LZCNT, so on Windows AVX2 stands for it, as it does in install.ps1.
    case " $flags " in *" abm "*) flags="$flags lzcnt" ;; esac
    if [ "$OS" = windows ]; then
        case " $flags " in *" avx2 "*) flags="$flags lzcnt" ;; esac
    fi
    for f in avx2 bmi1 lzcnt popcnt; do
        case " $flags " in *" $f "*) ;; *) LACKS="$LACKS $f" ;; esac
    done
    LACKS=$(printf '%s' "$LACKS" | tr '[:lower:]' '[:upper:]' | sed 's/^ //; s/ /, /g')
    case "$LACKS" in
        "") LEVEL=avx2 ;;
        *POPCNT*) LEVEL=none ;;
        *) LEVEL=generic ;;
    esac
    log "x86-64 features missing: ${LACKS:-none}"
}

confirm_system() {
    step "This system"
    if [ "$ARCH" != x86_64 ]; then
        BUILD=native
        say "$(describe_system), which Cadence has one build for."
        return
    fi
    x86_level
    case "$LEVEL" in
        avx2) say "$(describe_system), with AVX2, BMI1, LZCNT and POPCNT: the AVX2 build runs here." ;;
        generic) say "$(describe_system), without $LACKS: the generic build runs here, for any x86-64 with POPCNT." ;;
        unread) say "$(describe_system), whose processor features could not be read: the generic build, which needs POPCNT, is tried." ;;
        none)
            say "$(describe_system), without POPCNT, which both of Cadence's x86-64 builds need."
            fail "this processor cannot run Cadence"
            ;;
    esac
    if [ "$BUILD" = avx2 ] && [ "$LEVEL" != avx2 ]; then
        [ -n "$LACKS" ] || fail "--build avx2 needs AVX2, BMI1, LZCNT and POPCNT, and this processor's features could not be read"
        fail "--build avx2 needs AVX2, BMI1, LZCNT and POPCNT, and this processor lacks $LACKS"
    elif [ -n "$BUILD" ]; then
        say "The $BUILD build, as asked."
    elif [ "$LEVEL" = avx2 ]; then
        BUILD=avx2
        say "--generic takes the generic build instead, for any x86-64 with POPCNT."
    else
        BUILD=generic
    fi
}

# --- The release binary ------------------------------------------------------------------------

# The release's name for this system's binary, or nothing where no release builds one.
release_name() {
    case "$OS-$ARCH-$BUILD" in
        linux-x86_64-avx2) printf 'linux-x86-64-avx2' ;;
        linux-x86_64-generic) printf 'linux-x86-64-generic' ;;
        linux-aarch64-native) printf 'linux-aarch64' ;;
        macos-aarch64-native) printf 'macos-arm64' ;;
        macos-x86_64-avx2) printf 'macos-x86-64' ;;
        windows-x86_64-avx2) printf 'windows-x86-64-avx2' ;;
        windows-x86_64-generic) printf 'windows-x86-64-generic' ;;
    esac
}

# GitHub's latest release is the newest published one that is not a candidate, so its files can be read.
find_release() {
    step "Finding the release"
    if [ -n "$VERSION" ]; then
        say "Release $VERSION, as asked."
        return 0
    fi
    local tag=""
    log "\$ download $API_URL/latest"
    if download "$API_URL/latest" "$WORK/latest.json" 2> "$OUT"; then
        tag=$(grep -o '"tag_name":[[:space:]]*"[^"]*"' "$WORK/latest.json" | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/')
    fi
    redact < "$OUT" >> "$LOG"
    if ! printf '%s' "$tag" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+'; then
        WHY="GitHub names no published release$(said), so there is no binary to install."
        return 1
    fi
    VERSION=$tag
    say "The latest release is $VERSION."
}

# The SHA-256 the release's SHA256SUMS gives a file, or nothing.
listed() {
    awk -v f="$1" '$2 == f || $2 == "*" f {print $1; exit}' "$WORK/SHA256SUMS"
}

# Leaves the checked binary in BUILT, or the reason none is used in WHY.
try_binary() {
    local name file want got which
    name=$(release_name)
    if [ -z "$name" ]; then
        which=$(describe_system)
        [ "$ARCH" != x86_64 ] || which="$which in the $BUILD build"
        WHY="Cadence's releases have no binary for $which."
        return 1
    fi
    file="cadence-$VERSION-$name$EXE"
    log "\$ download $ASSETS_URL/$VERSION/SHA256SUMS"
    if ! download "$ASSETS_URL/$VERSION/SHA256SUMS" "$WORK/SHA256SUMS" 2> "$OUT"; then
        redact < "$OUT" >> "$LOG"
        WHY="Release $VERSION publishes no checksum file$(said), so none of its binaries is used."
        return 1
    fi
    want=$(listed "$file")
    if [ -z "$want" ]; then
        WHY="Release $VERSION's SHA256SUMS lists no $file, so there is no binary for this system."
        return 1
    fi
    say "Release $VERSION has a binary for this system: $file."

    step "Checking $file"
    log "\$ download $ASSETS_URL/$VERSION/$file"
    if ! download "$ASSETS_URL/$VERSION/$file" "$WORK/$file" 2> "$OUT"; then
        redact < "$OUT" >> "$LOG"
        WHY="Could not download $file$(said), so it is not used."
        return 1
    fi
    got=$(checksum "$WORK/$file")
    log "$file: SHA-256 $got; SHA256SUMS: $want"
    if [ "$got" != "$want" ]; then
        WHY="$file's SHA-256 is $got and SHA256SUMS says $want, so it is not used."
        return 1
    fi
    say "Its SHA-256 matches the release's SHA256SUMS."
    want=$(listed bench.txt)
    if [ -z "$want" ]; then
        WHY="Release $VERSION's SHA256SUMS lists no bench.txt to check $file against, so it is not used."
        return 1
    fi
    log "\$ download $ASSETS_URL/$VERSION/bench.txt"
    if ! download "$ASSETS_URL/$VERSION/bench.txt" "$WORK/bench.txt" 2> "$OUT"; then
        redact < "$OUT" >> "$LOG"
        WHY="Could not download the release's bench.txt$(said), so $file is not used."
        return 1
    fi
    if [ "$(checksum "$WORK/bench.txt")" != "$want" ]; then
        WHY="The release's bench.txt does not match SHA256SUMS, so $file is not used."
        return 1
    fi
    chmod 755 "$WORK/$file" || fail "cannot make $WORK/$file executable"
    if ! bench_and_name "$WORK/$file" "$(tr -d '[:space:]' < "$WORK/bench.txt")" "Cadence $VERSION"; then
        WHY="I${WHY#i}, so it is not used."
        return 1
    fi
    BUILT="$WORK/$file"
}

# --- The build ---------------------------------------------------------------------------------

offer_source() {
    if [ "$SOURCE" = 1 ]; then
        step "Building from source, as --source asks"
    else
        say "$WHY"
        step "Building from source instead"
    fi
    say "Building takes several minutes and about 2 GB of disk space, for the source, the Rust"
    say "version Cadence pins if it is not installed yet, and the build. Both are estimates."
    mkdir -p "$DATA_DIR" || fail "cannot create $DATA_DIR"
    local free what="Cadence's latest release"
    free=$(df -Pk "$DATA_DIR" 2>/dev/null | awk 'NR == 2 {print int($4 / 1024)}')
    if [ -n "$free" ]; then
        say "$(shown "$DATA_DIR") has $free MB free."
        [ "$free" -ge 2048 ] || say "That may not be enough."
    fi
    [ -z "$VERSION" ] || what="Cadence $VERSION"
    ask "Build $what from source?" y || stop "Nothing was installed."
}

check_git() {
    if command -v git > /dev/null 2>&1; then
        log "git: $(git --version)"
        return
    fi
    say "Git is needed to fetch Cadence's source, and it is not installed."
    case "$OS" in
        linux) say "Install it with your package manager, for example: sudo apt-get install git" ;;
        macos) say "Install it with Apple's command line tools: xcode-select --install" ;;
        windows) say "Install Git for Windows from https://git-scm.com/download/win and run this again in Git Bash." ;;
    esac
    fail "git is not installed"
}

msvc_version() {
    local program_files vswhere component=Microsoft.VisualStudio.Component.VC.Tools.x86.x64
    [ "$ARCH" = x86_64 ] || component=Microsoft.VisualStudio.Component.VC.Tools.ARM64
    program_files=$(cygpath -u "${PROGRAMFILES:-C:\\Program Files}")
    vswhere="$program_files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
    [ -x "$vswhere" ] || return 0
    "$vswhere" -latest -products '*' -requires "$component" -property installationVersion 2>/dev/null \
        | tr -d '\r' | head -n 1
}

check_linker() {
    case "$OS" in
        linux)
            if command -v cc > /dev/null 2>&1; then
                log "linker: $(cc --version 2>&1 | head -n 1)"
                return
            fi
            say "Rust needs a C compiler to link with, and none is installed."
            local sudo=sudo install=""
            [ "$(id -u)" != 0 ] || sudo=""
            if command -v apt-get > /dev/null 2>&1; then
                install="$sudo apt-get install -y build-essential"
            elif command -v dnf > /dev/null 2>&1; then
                install="$sudo dnf install -y gcc"
            elif command -v pacman > /dev/null 2>&1; then
                install="$sudo pacman -S --needed --noconfirm gcc"
            fi
            [ -n "$install" ] || fail "no C compiler: install gcc or clang with your package manager, then run this again"
            ask "Install one now with: $install ?" y || stop "Install gcc or clang, then run this again."
            if command -v apt-get > /dev/null 2>&1; then run $sudo apt-get update || fail "apt-get update failed"; fi
            run $install || fail "the compiler's install failed"
            command -v cc > /dev/null 2>&1 || fail "there is still no C compiler after the install"
            log "linker: $(cc --version 2>&1 | head -n 1)"
            ;;
        macos)
            if xcode-select -p > /dev/null 2>&1; then
                log "linker: $(cc --version 2>&1 | head -n 1)"
                return
            fi
            say "Rust needs Apple's command line tools to link with, and they are not installed."
            ask "Open Apple's installer for them now?" y || stop "Install them with: xcode-select --install, then run this again."
            run xcode-select --install
            stop "Finish the installer Apple opened, then run this script again."
            ;;
        windows)
            local found
            found=$(msvc_version)
            if [ -n "$found" ]; then
                log "linker: Visual Studio C++ build tools $found"
                return
            fi
            say "Rust needs Microsoft's C++ build tools to link with, and they are not installed."
            local tools="winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-package-agreements --accept-source-agreements --override \"--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended\""
            command -v winget > /dev/null 2>&1 \
                || fail "winget is not available: install the Visual Studio Build Tools with C++ from https://visualstudio.microsoft.com/visual-cpp-build-tools/, then run this again"
            ask "Install them now with winget (several GB, and Windows may ask for permission)?" y \
                || stop "Install the Visual Studio Build Tools with C++, then run this again."
            run winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-package-agreements \
                --accept-source-agreements \
                --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended" \
                || fail "the build tools' install failed: $tools"
            found=$(msvc_version)
            [ -n "$found" ] || fail "the C++ build tools are still not found after the install"
            log "linker: Visual Studio C++ build tools $found"
            ;;
    esac
}

install_rustup() {
    if [ "$OS" = windows ]; then
        local arch=x86_64
        [ "$ARCH" = x86_64 ] || arch=aarch64
        run download "https://win.rustup.rs/$arch" "$WORK/rustup-init.exe" || fail "could not download rustup-init.exe"
        run "$WORK/rustup-init.exe" -y --default-toolchain none || fail "rustup's install failed"
    else
        command -v curl > /dev/null 2>&1 || fail "curl is needed to install rustup; install curl, or Rust from https://rustup.rs"
        run sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none" \
            || fail "rustup's install failed"
    fi
    PATH="$CARGO_BIN:$PATH"
    command -v rustup > /dev/null 2>&1 || fail "rustup is still not found after its install"
}

check_rust() {
    [ ! -d "$CARGO_BIN" ] || PATH="$CARGO_BIN:$PATH"
    if command -v rustup > /dev/null 2>&1; then
        log "rustup: $(rustup --version 2>/dev/null | head -n 1)"
        return
    fi
    if command -v cargo > /dev/null 2>&1; then
        say "This Rust came without rustup, as a package manager's does, so it cannot fetch the Rust version Cadence pins."
    else
        say "Rust is not installed."
    fi
    choose "How would you like to go on?" \
        "Install rustup now, from rust-lang.org" \
        "Show the steps to install it by hand" \
        "Quit"
    case "$CHOICE" in
        1) install_rustup ;;
        2)
            if [ "$OS" = windows ]; then
                say "1. Download rustup-init.exe from https://rustup.rs and run it."
            else
                say "1. Run the command https://rustup.rs shows."
            fi
            say "2. Open a new terminal, so cargo is on your PATH."
            say "3. Run this script again. The Rust version Cadence pins installs at its first build."
            stop "Nothing was installed."
            ;;
        *) stop "Nothing was installed." ;;
    esac
}

# The tag the source is fetched at: the version asked for or found, or with neither the highest x.y.z.
resolve_tag() {
    local tags
    log "\$ git ls-remote --tags --refs $REPO_URL"
    tags=$(git ls-remote --tags --refs "$REPO_URL" 2> "$OUT") || {
        redact < "$OUT" >> "$LOG"
        fail "could not list the releases at $REPO_URL"
    }
    tags=$(printf '%s\n' "$tags" | sed -n 's#.*refs/tags/##p')
    if [ -z "$VERSION" ]; then
        VERSION=$(printf '%s\n' "$tags" | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' \
            | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
        [ -n "$VERSION" ] || fail "there is no release at $REPO_URL"
        say "The latest release is $VERSION."
    elif ! printf '%s\n' "$tags" | grep -Fqx -- "$VERSION"; then
        case "$VERSION" in
            *+*) fail "there is no test base $VERSION at $REPO_URL" ;;
            *) fail "there is no release $VERSION at $REPO_URL" ;;
        esac
    fi
}

fetch_source() {
    step "Fetching Cadence $VERSION"
    SRC="$WORK/cadence"
    rm -rf "$SRC"
    run git -c init.defaultBranch=main init --quiet "$SRC" || fail "could not create $SRC"
    run git -C "$SRC" remote add origin "$REPO_URL" || fail "could not name the repository in $SRC"
    # A release names itself from its annotated tag, so the tag is fetched as itself.
    run git -C "$SRC" fetch --quiet --depth 1 origin "+refs/tags/$VERSION:refs/tags/$VERSION" \
        || fail "could not fetch Cadence $VERSION"
    run git -C "$SRC" -c advice.detachedHead=false checkout --quiet --detach "refs/tags/$VERSION" \
        || fail "could not check out Cadence $VERSION"
}

prepare_toolchain() {
    step "The Rust toolchain"
    (cd "$SRC" && run rustup toolchain install) \
        || say "This rustup installs the Rust version Cadence pins as the build starts instead."
    log "rustc: $(cd "$SRC" && rustc --version 2>&1)"
    log "cargo: $(cd "$SRC" && cargo --version 2>&1)"
    TRIPLE=$(cd "$SRC" && rustc -vV 2>/dev/null | sed -n 's/^host: //p')
    [ -n "$TRIPLE" ] || fail "rustc does not name the system it builds for"
    log "target: $TRIPLE"
}

build() {
    step "Building Cadence $VERSION"
    local args="build --release --locked -p cadence-engine --bin cadence"
    case "$BUILD" in
        generic)
            if grep -q "^\[target\.$TRIPLE\]" "$SRC/.cargo/x86-64-generic.toml" 2>/dev/null; then
                args="$args --config .cargo/x86-64-generic.toml"
            else
                say "Cadence $VERSION has no generic settings for $TRIPLE, so it is built for that system's baseline."
            fi
            ;;
        avx2)
            grep -q "^\[target\.$TRIPLE\]" "$SRC/.cargo/config.toml" \
                || say "Cadence $VERSION has no AVX2 settings for $TRIPLE, so it is built for that system's baseline."
            ;;
    esac
    # RUSTFLAGS would replace the repository's own per-target flags, so it is left out of the build.
    (cd "$SRC" && run env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS cargo $args) || fail "the build failed"
    BUILT="$SRC/target/release/cadence$EXE"
    [ -f "$BUILT" ] || fail "the build left no binary at $BUILT"
}

id_name() {
    printf 'uci\nquit\n' | "$1" 2>/dev/null | tr -d '\r' | sed -n 's/^id name //p' | head -n 1
}

# Runs a binary's bench and asks its name, leaving the reason in WHY if either is not the one wanted.
bench_and_name() {
    local bin=$1 want=$2 name=$3 rc got first
    say "Running its bench."
    log "\$ cadence bench"
    (cd "$WORK" && "$bin" bench) > "$WORK/bench.out" 2> "$OUT"
    rc=$?
    redact < "$OUT" >> "$LOG"
    got=$(tr -d '\r' < "$WORK/bench.out" | tail -n 1 | awk '{print $1}')
    log "bench: exit $rc, ${got:-nothing}; bench.txt: $want"
    if [ "$rc" -ne 0 ]; then
        first=$(head -n 1 "$OUT" | tr -d '\r')
        # 128 plus SIGILL's 4.
        [ "$rc" -ne 132 ] || first="an illegal instruction, one this processor lacks"
        WHY="it stopped in its bench with exit $rc${first:+ ($first)}"
        return 1
    fi
    if [ -z "$got" ] || [ "$got" != "$want" ]; then
        WHY="its bench counts ${got:-nothing} and bench.txt says $want"
        return 1
    fi
    ID=$(id_name "$bin")
    log "id name: ${ID:-nothing}"
    if [ "$ID" != "$name" ]; then
        WHY="it calls itself '${ID:-nothing}' and not '$name'"
        return 1
    fi
    say "The bench counts $got nodes, as bench.txt says, and the engine calls itself $ID."
}

# A release calls itself by its version; a test base's build is the version's -dev- at its commit.
verify() {
    step "Checking the build"
    local name="Cadence $VERSION"
    case "$VERSION" in
        *+*) name="Cadence ${VERSION%%+*}-dev-$(git -C "$SRC" rev-parse HEAD | cut -c1-7)" ;;
    esac
    bench_and_name "$BUILT" "$(tr -d '[:space:]' < "$SRC/bench.txt")" "$name" \
        || fail "$WHY, so it is not installed"
}

# --- The install -------------------------------------------------------------------------------

install_binary() {
    step "Installing"
    local dest="" old previous
    previous=$(state_get binary)
    if [ -n "$PREFIX" ]; then
        dest=$PREFIX
        say "Installing into $dest, as --dir asks."
    elif [ "$MODE" = update ] && [ -n "$previous" ]; then
        dest=$(dirname "$previous")
        say "Installing into $(shown "$dest"), where Cadence was installed before."
    else
        dest=$DEFAULT_PREFIX
        say "Installing into $(shown "$dest"), the default folder; --dir DIR installs into DIR instead."
    fi
    mkdir -p "$dest" || fail "cannot create $dest"
    TARGET="$dest/cadence$EXE"
    if [ -e "$TARGET" ]; then
        old=$(id_name "$TARGET")
        say "There is already a Cadence at $(shown "$TARGET")${old:+, calling itself $old}."
        local default=n
        [ "$MODE" != update ] || default=y
        ask "Replace it?" "$default" || stop "Kept the one there; nothing was installed."
    fi
    { cp "$BUILT" "$TARGET.new" && chmod 755 "$TARGET.new" && mv -f "$TARGET.new" "$TARGET"; } \
        || { rm -f "$TARGET.new"; fail "could not write $TARGET; if Cadence is running, close it and run this again"; }
    state_set binary "$TARGET"
    say "Installed $(shown "$TARGET")."
}

shell_rc() {
    case "${SHELL##*/}" in
        zsh) printf '%s' "$HOME/.zshrc" ;;
        bash) if [ "$OS" = macos ]; then printf '%s' "$HOME/.bash_profile"; else printf '%s' "$HOME/.bashrc"; fi ;;
        *) printf '%s' "$HOME/.profile" ;;
    esac
}

# A folder named with --dir is the caller's to manage, so the offer defaults to no there.
offer_path() {
    local dest default=y
    dest=$(dirname "$TARGET")
    [ -z "$PREFIX" ] || default=n
    case ":$PATH:" in
        *":$dest:"*)
            say "$(shown "$dest") is already on your PATH."
            return
            ;;
    esac
    if [ "$OS" = windows ]; then
        local win quoted
        win=$(cygpath -w "$dest")
        ask "Add $win to your PATH, so a terminal finds cadence?" "$default" || return 0
        quoted=${win//\'/\'\'}
        run powershell.exe -NoProfile -Command "\$p = [Environment]::GetEnvironmentVariable('Path', 'User'); if (-not \$p) { \$p = '' }; if ((\$p -split ';') -notcontains '$quoted') { [Environment]::SetEnvironmentVariable('Path', (\$p.TrimEnd(';') + ';$quoted').TrimStart(';'), 'User') }" \
            || fail "could not add $win to your PATH"
        state_set path_windows "$win"
        added_to_path "export PATH=\"$dest:\$PATH\""
        say "If a new Windows Terminal tab still does not find cadence, close all of its windows and start it again."
    else
        local rc line
        rc=$(shell_rc)
        line="export PATH=\"$dest:\$PATH\""
        ask "Add $dest to your PATH in $rc, so a terminal finds cadence?" "$default" || return 0
        printf '\n%s\n%s\n' "$MARK" "$line" >> "$rc" || fail "could not write $rc"
        state_set path_rc "$rc"
        state_set path_line "$line"
        added_to_path "$line"
    fi
}

# Each command on a line of its own, so it can be copied whole.
added_to_path() {
    say "Added. This terminal will not find cadence until you open a new one. To run it here now:"
    case "$TARGET" in *" "*) say "\"$TARGET\"" ;; *) say "$TARGET" ;; esac
    say "or put it on this terminal's PATH with:"
    say "$1"
}

offer_tables() {
    step "Syzygy tablebases, if you want them"
    say "Cadence can read the 3-4-5 piece Syzygy tablebases, 290 files and about 1 GB, to play every"
    say "ending of five pieces or fewer perfectly. They are downloaded from $TABLES_URL."
    ask "Download them now?" n || { say "Skipped."; return 0; }
    local dir="$DATA_DIR/syzygy/3-4-5" sum file group count checked=0 bytes=0
    mkdir -p "$dir" || fail "cannot create $dir"
    run download "$TABLES_URL/sha256" "$WORK/sha256" || fail "could not download the tables' published checksums"
    awk '{n = $2; sub(/\.rtb[wz]$/, "", n); gsub(/v/, "", n); if (length(n) <= 5) print}' \
        "$WORK/sha256" > "$WORK/3-4-5.sha256"
    count=$(wc -l < "$WORK/3-4-5.sha256" | tr -d ' ')
    [ "$count" = 290 ] || fail "the published checksums list $count tables of five pieces or fewer, not 290"
    state_set tables "$dir"
    while read -r sum file; do
        case "$file" in
            *.rtbw) group=3-4-5-wdl ;;
            *.rtbz) group=3-4-5-dtz ;;
            *) fail "the published checksums name $file, which is not a table" ;;
        esac
        if [ ! -f "$dir/$file" ] || [ "$(checksum "$dir/$file")" != "$sum" ]; then
            download "$TABLES_URL/$group/$file" "$dir/$file.part" || fail "could not download $file"
            [ "$(checksum "$dir/$file.part")" = "$sum" ] \
                || { rm -f "$dir/$file.part"; fail "$file does not match its published checksum"; }
            mv -f "$dir/$file.part" "$dir/$file" || fail "could not write $dir/$file"
        fi
        checked=$((checked + 1))
        bytes=$((bytes + $(wc -c < "$dir/$file")))
        [ $((checked % 29)) -ne 0 ] \
            || say "  $checked of 290 downloaded and checked, $((bytes / 1048576)) of $((TABLES_BYTES / 1048576)) MB"
    done < "$WORK/3-4-5.sha256"
    say "All 290 tables are in $(shown "$dir"), each matching its published checksum."
    say "To use them, set the engine option SyzygyPath in your chess program to this folder:"
    say "$(shown "$dir")"
    [ "$OS" != macos ] \
        || say "Cadence keeps each table open, and macOS allows a program 256 open files: start your chess program from a terminal after 'ulimit -n 1024'."
}

afterwards() {
    step "Done"
    say "Cadence is at $(shown "$TARGET") and calls itself $ID."
    say ""
    say "To play it in a chess program, add it as a UCI engine and choose that file:"
    say "  Arena: Engines, Install New Engine, choose the file, then UCI when asked."
    say "  Cute Chess: Tools, Settings, Engines, the + button, the file as the command, UCI as the protocol."
    say "  Banksia: add an engine in its engine manager and choose the file."
    [ "$WSL" = 0 ] || say "This is a Linux build in WSL, for chess programs in WSL; a Windows one needs install.ps1."
    say ""
    say "Report bugs as GitHub issues: $ISSUES_URL"
}

uninstall() {
    step "Uninstalling Cadence"
    local bin rc line win tables
    bin=$(state_get binary)
    [ -n "$bin" ] || bin="${PREFIX:-$DEFAULT_PREFIX}/cadence$EXE"
    if [ -e "$bin" ]; then
        if ask "Remove $(shown "$bin")?" y; then
            rm -f "$bin" || fail "could not remove $bin; if Cadence is running, close it and run this again"
            say "Removed it."
        fi
    else
        say "There is no Cadence at $(shown "$bin")."
    fi
    rc=$(state_get path_rc)
    line=$(state_get path_line)
    if [ -n "$rc" ] && [ -f "$rc" ] && ask "Remove the PATH line this script added to $rc?" y; then
        CADENCE_LINE=$line CADENCE_MARK=$MARK awk '$0 != ENVIRON["CADENCE_LINE"] && $0 != ENVIRON["CADENCE_MARK"]' \
            "$rc" > "$rc.cadence" && cat "$rc.cadence" > "$rc" && rm -f "$rc.cadence" || fail "could not rewrite $rc"
        say "Removed it."
    fi
    win=$(state_get path_windows)
    if [ -n "$win" ] && ask "Remove $win from your PATH, which this script added?" y; then
        local quoted=${win//\'/\'\'}
        run powershell.exe -NoProfile -Command "\$p = [Environment]::GetEnvironmentVariable('Path', 'User'); if (\$p) { [Environment]::SetEnvironmentVariable('Path', ((\$p -split ';') | Where-Object { \$_ -and \$_ -ne '$quoted' }) -join ';', 'User') }" \
            || fail "could not remove $win from your PATH"
        say "Removed it."
    fi
    tables=$(state_get tables)
    if [ -n "$tables" ] && [ -d "$tables" ] && ask "Remove the tablebases at $(shown "$tables"), about 1 GB?" y; then
        rm -rf "$tables" || fail "could not remove $tables"
        say "Removed them."
    fi
    rm -f "$STATE"
    say "Done."
}

# --- The run -----------------------------------------------------------------------------------

system_report
if [ "$MODE" = uninstall ]; then
    uninstall
    exit 0
fi
step "Cadence's installer"
say "This installs Cadence's release binary for this system once its checksum and bench match the"
say "release's, or builds it from source where no binary fits. It asks before building, installing a"
say "compiler or Rust, replacing an install, changing your PATH or downloading tablebases."
check_version
mkdir -p "$WORK" || fail "cannot create $WORK"
confirm_system
[ "$SOURCE" = 1 ] || { find_release && try_binary; }
if [ -z "$BUILT" ]; then
    offer_source
    check_git
    resolve_tag
    check_linker
    check_rust
    fetch_source
    prepare_toolchain
    build
    verify
fi
install_binary
offer_path
offer_tables
afterwards
