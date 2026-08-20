#!/usr/bin/env bash
# Piper TTS Reader - portable Linux launcher.
#
# Everything the app needs ships in this folder: the app binary, the bundled
# piper runtime and the voice models. Uncompress the tarball and run:
#
#     ./run.sh
#
# The only system-level dependency is WebKitGTK 4.1, which Tauri requires and
# cannot be bundled into the tarball; the script checks for it and prints the
# install command instead of crashing.

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
# bundled; detect the distribution and print the exact install command for it
# instead of failing with an incomprehensible error.
if ! ldconfig -p 2>/dev/null | grep -q 'libwebkit2gtk-4.1.so'; then
    echo "Piper TTS Reader needs WebKitGTK 4.1, but it is not installed." >&2
    echo "Install it with your package manager, then run this script again:" >&2
    echo >&2
    if [ -r /etc/os-release ]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        case "${ID_LIKE:-$ID}" in
            *debian* | *ubuntu*)
                echo "  Debian / Ubuntu:" >&2
                echo "    sudo apt install libwebkit2gtk-4.1-0" >&2
                ;;
            *fedora* | *rhel* | *centos*)
                echo "  Fedora / RHEL:" >&2
                echo "    sudo dnf install webkit2gtk4.1" >&2
                ;;
            *arch*)
                echo "  Arch / Manjaro:" >&2
                echo "    sudo pacman -S webkit2gtk-4.1" >&2
                ;;
            *)
                echo "  Your distribution (${PRETTY_NAME:-unknown}):" >&2
                echo "    install the 'webkit2gtk-4.1' package (or the equivalent WebKitGTK 4.1 runtime)" >&2
                ;;
        esac
    else
        echo "  Debian / Ubuntu:  sudo apt install libwebkit2gtk-4.1-0" >&2
        echo "  Fedora / RHEL:    sudo dnf install webkit2gtk4.1" >&2
        echo "  Arch / Manjaro:   sudo pacman -S webkit2gtk-4.1" >&2
    fi
    exit 1
fi

if [ ! -x "$APP" ]; then
    echo "Piper TTS Reader binary not found: $APP" >&2
    exit 1
fi

# Mirror the Windows portable layout: launch from the app folder so relative
# lookups (piper's tashkeel model, etc.) behave identically to the zip.
cd "$DIR"
exec "$APP" "$@"
