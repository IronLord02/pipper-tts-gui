# Piper TTS Reader

A portable, fully offline text-to-speech reader for Windows and Linux. Paste text or load a `.txt` file, pick a voice, and synthesize it to a WAV — no internet, no accounts, no installation.

**⬇ Download the latest release:** [Releases](https://github.com/IronLord02/pipper-tts-gui/releases)

> Ships as a self-contained folder (app + Piper runtime + voice models). Runs from a USB stick. No installation required.

## What it does

- **Offline text-to-speech**: no downloads inside the app, no proxy, no network calls.
- **Voice auto-detection**: every Piper voice found in the models folder is listed with language, model name, size and quality.
- **Models folder picker**: point the app at any folder with `.onnx` + `.onnx.json` pairs; the choice is remembered between sessions.
- **Batch synthesis queue**: queue multiple texts and synthesize them in one go.
- **Embedded MP3 conversion**: output WAVs can be converted to MP3 without external tools.
- **Duration estimate & progress indicator**: shows roughly how long the audio and synthesis will take before you start.
- **Paste from clipboard** or **load a `.txt` file**; save audio as WAV anywhere.

## Tech Stack

`Rust` `Tauri v2` `TypeScript` `Vite` `Piper (CLI)` `WebKitGTK`

## How it works

The app embeds a Vite + TypeScript frontend inside a Tauri (Rust) binary. Synthesis runs the bundled `piper` CLI (`piper.exe` on Windows) with the selected `.onnx` model, feeding text on stdin and reading the produced WAV. The Piper runtime and models folder are discovered next to the running executable (or recursively in subfolders), which is what makes the app portable from a USB stick. The console window of `piper.exe` is suppressed (`CREATE_NO_WINDOW`), so all feedback happens inside the app UI.

## How to run

### Pre-built releases
- **Windows**: download `pipper-tts-reader-windows.zip`, extract anywhere, run `Piper TTS Reader.exe`.
- **Linux (x86_64)**: download `Piper-TTS-Reader-linux-x86_64.tar.gz`, extract, run `./run.sh`. `run.sh` detects your distro (Debian/Ubuntu, Fedora/RHEL, Arch/Manjaro) and installs WebKitGTK 4.1 automatically.

### Build from source
```bash
# Prerequisites: Node.js (LTS), Rust (stable), Tauri v2 prerequisites
npm install
npm run tauri dev          # development
npm run tauri build -- --no-bundle   # release binary
```
The release binary is produced at `src-tauri/target/release/app` (Linux) or `app.exe` (Windows).

## Tests

```bash
cargo test
```

## Included voices

The release archives ship with: Spanish, English, Portuguese (BR), German, French, Russian, and Chinese (Mandarin). Any [Piper voice](https://huggingface.co/rhasspy/piper-voices/tree/main) can be added by dropping its `.onnx` and `.onnx.json` files into a folder and selecting it from the app.

## Acknowledgments

Piper is released under the MIT license by the Rhasspy project. Individual voice models are released under their own licenses — see each model's `MODEL_CARD` / `ALIASES` files.

## Contact

**Developer**: [IronLord02](https://github.com/IronLord02)
**Status**: Open to remote freelance opportunities.
