# Piper TTS Reader

A portable, fully offline text-to-speech reader for Windows, built with
[Tauri v2](https://tauri.app) and [Piper](https://github.com/rhasspy/piper).

Paste text or load a `.txt` file, pick a voice, and synthesize it to a WAV
file — no internet, no accounts, no installation.

## Download

Grab the latest release: [Releases](https://github.com/IronLord02/pipper-tts-gui/releases)

1. Download `pipper-tts-reader-windows.zip`
2. Extract it anywhere (a USB stick works fine)
3. Run `Piper TTS Reader.exe`

No installation required. Works completely offline.

## Features

- **Offline text-to-speech** — no downloads inside the app, no proxy
- **Voice auto-detection** — every voice found in the models folder is listed
  with its language, model name, size and quality
- **Models folder picker** — point the app at any folder with voice models;
  the choice is remembered between sessions
- **Duration estimate** — shows roughly how long the audio will be and how
  long synthesis will take before you start
- **Progress indicator** — animated progress bar while synthesizing, with a
  clear "done" message
- **Paste from clipboard** or **load a `.txt` file**
- **Save audio as WAV** anywhere you want

## Included voices

The release ZIP ships with:

| Language | Voice | Quality |
| -------- | ----- | ------- |
| English  | `en_US-danny-low` | low |
| Spanish  | `es_ES-carlfm-x_low` | x_low |

## Adding more voices

The app can use any [Piper voice](https://huggingface.co/rhasspy/piper-voices/tree/main):

1. Go to the voice model you want (e.g. `es/es_ES`, `en/en_US`, `fr/fr_FR`)
2. Download the **two files** for the voice:
   - `<voice>.onnx`
   - `<voice>.onnx.json`
3. Put both files together in a folder. Subfolders are fine, for example:

   ```
   models/
   ├── EN/
   │   ├── en_US-danny-low.onnx
   │   └── en_US-danny-low.onnx.json
   └── ES/
       ├── es_ES-carlfm-x_low.onnx
       └── es_ES-carlfm-x_low.onnx.json
   ```

4. In the app, click **Choose models folder...** and select that folder.
   The app detects every complete voice pair automatically.

> **Important:** both the `.onnx` and the `.onnx.json` must be next to each
> other. The `.json` config is required.

## Folder layout

```
pipper-tts-reader/
├── Piper TTS Reader.exe     <- the app
├── WebView2Loader.dll       <- required runtime DLL
├── models/                  <- voice models (subfolders per language)
│   ├── EN/...
│   └── ES/...
├── piper-runtime/           <- Piper CLI + dependencies (required)
│   ├── piper.exe
│   ├── onnxruntime.dll
│   ├── espeak-ng.dll
│   ├── piper_phonemize.dll
│   ├── libtashkeel_model.ort
│   └── espeak-ng-data/
└── output/                  <- generated WAVs (created automatically)
```

`output/` is created next to the executable on first synthesis, so generated
audio never mixes with the models folder. You can delete it any time.

## Building from source

### Prerequisites

- [Node.js](https://nodejs.org) (LTS)
- [Rust](https://rustup.rs) (stable)
- Tauri v2 prerequisites for Windows
  ([WebView2](https://developer.microsoft.com/microsoft-edge/webview2/),
  MSVC or GNU toolchain)

### Commands

```bash
# Install frontend dependencies
npm install

# Run in development
npm run tauri dev

# Build the release executable (no installer)
npm run tauri build -- --no-bundle
```

The release binary is produced at `src-tauri/target/release/app.exe`.

### Tests

```bash
cargo test
```

### Bundling the portable ZIP

1. Build with `npm run tauri build -- --no-bundle`
2. Copy the app next to the runtime and models:

   ```
   dist/
   ├── Piper TTS Reader.exe
   ├── WebView2Loader.dll
   ├── models/          <- your voice models
   └── piper-runtime/   <- piper.exe + dlls + espeak-ng-data
   ```

3. Zip the folder and publish it as a GitHub release.

> **Tip:** remove `piper-tts-settings.json` (created after the first run) and
> the `output/` folder before zipping — they are per-user state.

## How it works

- The app embeds the frontend (Vite + TypeScript) inside the Tauri binary.
- Synthesis runs the bundled `piper.exe` with the selected `.onnx` model and
  its `.onnx.json` config, feeding the text on stdin and reading the produced
  WAV.
- The Piper runtime and the models folder are discovered next to the running
  executable (or recursively in subfolders), which is what makes the app
  portable from a USB stick.
- The console window of `piper.exe` is suppressed (`CREATE_NO_WINDOW`), so the
  only feedback is inside the app UI.

## Acknowledgments

Piper is released under the MIT license by the Rhasspy project. Individual
voice models are released under their own licenses — see each model's
`MODEL_CARD` / `ALIASES` files.