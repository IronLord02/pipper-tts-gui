# Platform runtimes

Per-OS Piper runtime bundles. Keep each platform in its own folder so a build
never mixes binaries from another operating system.

| Folder | Target | Status |
|--------|--------|--------|
| `win-x86_64/` | Windows x64 | Lives in `test app cpiarl/piper-runtime` (working) |
| `linux-x86_64/` | Linux x64 (portable tarball) | Populated (working) |
| `macos-aarch64/` | macOS arm64 (future) | Not started |

The bundled `piper-runtime` next to the Windows exe stays untouched. The Linux
packaging workflow copies `linux-x86_64/` into the portable tarball as
`piper-runtime/` (next to the app binary, same layout as the Windows zip), per
`src-tauri/src/synth.rs` runtime resolution — never the reverse.

## Expected layout of `linux-x86_64/`

Derived from the official rhasspy/piper Linux tarball (`piper_linux_x86_64.tar.gz`).
Verify the exact file names when unpacking; the tarball layout can differ
between piper versions.

```
linux-x86_64/
  piper                      # Linux ELF binary
  libespeak-ng.so*           # shared libs shipped by the tarball
  libpiper_phonemize.so*
  onnxruntime*.so*           # if not included, fetch the matching onnxruntime
  espeak-ng-data/            # identical data, OS-independent
  models/                    # optional: bundled voices (the app can also use
                             # its own models folder picker)
```

The voice models (`.onnx` + `.onnx.json`) are pure data and work on every OS.

## Packaging

The GitHub workflow `.github/workflows/build-linux-tarball.yml` builds the app
with `--no-bundle`, copies this folder into the package as `piper-runtime/`,
copies `models/` as `models/`, adds `run.sh`, and produces
`Piper-TTS-Reader-linux-x86_64.tar.gz`. `run.sh` sets `LD_LIBRARY_PATH` so the
bundled piper finds its `.so` libraries.
