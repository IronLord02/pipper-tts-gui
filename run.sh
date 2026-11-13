#!/usr/bin/env bash
# Piper TTS Reader - portable Linux launcher.
#
# Everything the app needs ships in this folder: the app binary, the bundled
# piper runtime and the voice models. Uncompress the tarball and run:
#
#     ./run.sh
#
# The only system-level dependency is WebKitGTK 4.1, which Tauri requires and
# cannot be bundled into the tarball. This script detects your distribution,
# installs the dependency automatically (asking for your password when needed)
# and then launches the app binary.

set -euo pipefail

# Resolve the folder this script lives in, wherever the user extracted it.
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP="$DIR/piper-tts-reader"
RUNTIME="$DIR/piper-runtime"

# The bundled piper CLI loads libespeak-ng.so, libpiper_phonemize.so and
# libonnxruntime.so from its own folder. Linux does not search the executable's
# directory by default (unlike Windows), so point the dynamic linker at the
# runtime folder; the app inherits this environment when it spawns piper.
export LD_LIBRARY_PATH="$RUNTIME${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

# Tauri v2 on Linux requires WebKitGTK 4.1 at the system level. It cannot be
# bundled, so make sure it is installed before launching.

have_webkit() {
    ldconfig -p 2>/dev/null | grep -q 'libwebkit2gtk-4.1.so'
}

# Run a command with root privileges: directly when already root, via sudo
# otherwise.
as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    else
        sudo "$@"
    fi
}

# Detect the distribution family from /etc/os-release and install the correct
# WebKitGTK 4.1 runtime package for it. Package names follow the official
# Tauri v2 prerequisites (https://v2.tauri.app/start/prerequisites/):
#   Debian/Ubuntu : libwebkit2gtk-4.1-0
#   Arch/Manjaro  : webkit2gtk-4.1
#   Fedora/RHEL   : webkit2gtk4.1
install_webkit() {
    local family="" distro=""

    if [ -r /etc/os-release ]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        distro="${PRETTY_NAME:-${ID:-unknown}}"
        case "${ID_LIKE:-} ${ID:-}" in
            *debian* | *ubuntu*)
                family=debian
                ;;
            *arch*)
                family=arch
                ;;
            *fedora* | *rhel* | *centos*)
                family=fedora
                ;;
        esac
    fi

    case "$family" in
        debian)
            echo "Detected Debian-family system ($distro)."
            echo "Installing WebKitGTK 4.1 (package: libwebkit2gtk-4.1-0)..."
            # A failed refresh must not block the install; stale indexes are
            # reported by apt itself if they end up breaking it.
            as_root apt-get update || true
            as_root apt-get install -y libwebkit2gtk-4.1-0
            ;;
        arch)
            echo "Detected Arch-family system ($distro)."
            echo "Installing WebKitGTK 4.1 (package: webkit2gtk-4.1)..."
            as_root pacman -Sy --needed --noconfirm webkit2gtk-4.1
            ;;
        fedora)
            echo "Detected Fedora/RHEL-family system ($distro)."
            echo "Installing WebKitGTK 4.1 (package: webkit2gtk4.1)..."
            if command -v dnf >/dev/null 2>&1; then
                as_root dnf install -y webkit2gtk4.1
            else
                as_root yum install -y webkit2gtk4.1
            fi
            ;;
        *)
            echo "Piper TTS Reader needs WebKitGTK 4.1, which was not found." >&2
            echo "Could not detect a supported distribution (${distro:-unknown});" >&2
            echo "install it manually with your package manager, then run this script again:" >&2
            echo >&2
            echo "  Debian / Ubuntu:  sudo apt install libwebkit2gtk-4.1-0" >&2
            echo "  Fedora / RHEL:    sudo dnf install webkit2gtk4.1" >&2
            echo "  Arch / Manjaro:   sudo pacman -S webkit2gtk-4.1" >&2
            return 1
            ;;
    esac
}

if ! have_webkit; then
    echo "WebKitGTK 4.1 is missing; setting it up (a few seconds, may ask for your password)..."
    echo
    if ! install_webkit; then
        exit 1
    fi
    echo
    if ! have_webkit; then
        echo "Installation finished but WebKitGTK 4.1 is still not detected." >&2
        echo "Run this script again, or install the library manually." >&2
        exit 1
    fi
fi

if [ ! -x "$APP" ]; then
    echo "Piper TTS Reader binary not found: $APP" >&2
    exit 1
fi

# Mirror the Windows portable layout: launch from the app folder so relative
# lookups (piper's tashkeel model, etc.) behave identically to the zip.
cd "$DIR"

echo "All good! Everything is ready."
echo "Now launching the Piper TTS Reader executable below..."
echo

exec "$APP" "$@"
