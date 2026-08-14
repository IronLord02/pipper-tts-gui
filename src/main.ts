import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";

interface DurationEstimate {
  chars: number;
  estimated_audio_secs: number;
  estimated_process_secs: number;
}

interface SynthesisResult {
  wav_path: string;
  audio_secs: number;
  estimated_audio_secs: number;
  chars: number;
}

interface InstalledVoice {
  id: string;
  language: string;
  display_name: string;
  size_bytes: number;
  quality: string;
}

const app = document.querySelector<HTMLDivElement>("#app")!;

app.innerHTML = `
  <main class="shell">
    <h1>Piper TTS Reader</h1>
    <p class="muted">
      Synthesize text with your locally installed Piper voices.
    </p>

    <section class="panel convert-panel">
      <h2>Text to speech</h2>
      <textarea
        id="tts-text"
        aria-label="Text to synthesize"
        placeholder="Paste the text you want to hear..."
      ></textarea>
      <p class="counter">
        <span id="tts-chars">0</span> characters &middot; about
        <span class="estimate" id="tts-estimate">0.0</span>&nbsp;s of audio &middot; approx.
        <span class="estimate" id="tts-estimate-process">0.0</span>&nbsp;s to generate
      </p>

      <div class="progress" id="tts-progress" hidden>
        <div class="progress-fill"></div>
      </div>

      <div class="folder-picker">
        <div class="folder-picker-row">
          <label for="btn-choose-dir">Models folder</label>
          <span
            id="models-dir-label"
            class="models-dir-label"
            title=""
          >Loading...</span>
        </div>
        <div class="folder-picker-actions">
          <button id="btn-choose-dir" type="button">Choose models folder...</button>
          <button id="btn-reset-dir" type="button" class="secondary">Use bundled models</button>
        </div>
        <p id="dir-status" class="status" role="status"></p>
      </div>

      <div class="voice-picker">
        <div class="voice-picker-row">
          <span class="voice-picker-label">Voice</span>
          <span id="voice-active-label" class="voice-active" hidden>[Active]</span>
        </div>
        <select
          id="voice-select"
          class="voice-select"
          aria-label="Available voices"
        ></select>
      </div>

      <div class="actions">
        <button id="btn-paste" type="button">Paste from clipboard</button>
        <button id="btn-load" type="button">Load .txt file</button>
        <button id="btn-clear" type="button">Clear</button>
        <button id="btn-synthesize" type="button" class="primary">Synthesize</button>
        <button id="btn-save-as" type="button">Save WAV as...</button>
      </div>
      <input id="file-input" type="file" accept=".txt,text/plain" hidden />

      <p id="tts-status" class="status" role="status"></p>
      <dl class="result" id="tts-result" hidden>
        <dt>Audio</dt>
        <dd id="result-duration"></dd>
        <dt>File</dt>
        <dd id="result-path"></dd>
      </dl>
    </section>

    <section class="panel help-panel">
      <h2>Help</h2>
      <p>
        Piper voices are stored as pairs of files: a model (<code>.onnx</code>) and its
        settings file (<code>.onnx.json</code>). The app needs both files together before a
        voice can be used.
      </p>
      <h3>Where to get models</h3>
      <p>
        Browse the
        <a href="https://huggingface.co/rhasspy/piper-voices/tree/main" target="_blank" rel="noopener">
          official Piper voices repository
        </a>.
        Open a language folder (for example <code>es/es_ES</code> or <code>en/en_US</code>),
        then open a voice folder inside it and download the two files named
        <code>&lt;voice&gt;.onnx</code> and <code>&lt;voice&gt;.onnx.json</code>.
      </p>
      <h3>How to load them</h3>
      <p>
        Put the two files together in any folder on your computer (subfolders are fine), then
        click "Choose models folder..." and select that folder. The app will list every voice
        it detects.
      </p>
      <h3>Requirements</h3>
      <ul>
        <li>The <code>.onnx</code> and the <code>.onnx.json</code> files must be next to each other.</li>
        <li>The <code>.json</code> file is required; a voice is skipped when it is missing.</li>
      </ul>
    </section>
  </main>
`;

// ---- Conversion view ----

const textArea = document.querySelector<HTMLTextAreaElement>("#tts-text")!;
const charCount = document.querySelector<HTMLSpanElement>("#tts-chars")!;
const estimateEl = document.querySelector<HTMLSpanElement>("#tts-estimate")!;
const processEstimateEl = document.querySelector<HTMLSpanElement>("#tts-estimate-process")!;
const progressEl = document.querySelector<HTMLDivElement>("#tts-progress")!;
const ttsStatusEl = document.querySelector<HTMLParagraphElement>("#tts-status")!;
const resultBox = document.querySelector<HTMLDListElement>("#tts-result")!;
const resultDuration = document.querySelector<HTMLElement>("#result-duration")!;
const resultPath = document.querySelector<HTMLElement>("#result-path")!;
const pasteBtn = document.querySelector<HTMLButtonElement>("#btn-paste")!;
const loadBtn = document.querySelector<HTMLButtonElement>("#btn-load")!;
const clearBtn = document.querySelector<HTMLButtonElement>("#btn-clear")!;
const synthesizeBtn = document.querySelector<HTMLButtonElement>("#btn-synthesize")!;
const saveAsBtn = document.querySelector<HTMLButtonElement>("#btn-save-as")!;
const fileInput = document.querySelector<HTMLInputElement>("#file-input")!;
const chooseDirBtn = document.querySelector<HTMLButtonElement>("#btn-choose-dir")!;
const resetDirBtn = document.querySelector<HTMLButtonElement>("#btn-reset-dir")!;
const modelsDirLabel = document.querySelector<HTMLSpanElement>("#models-dir-label")!;
const dirStatusEl = document.querySelector<HTMLParagraphElement>("#dir-status")!;
const voiceSelect = document.querySelector<HTMLSelectElement>("#voice-select")!;
const voiceActiveLabel = document.querySelector<HTMLSpanElement>("#voice-active-label")!;

let selectedVoiceId: string | null = null;

function showDirStatus(message: string, ok: boolean): void {
  dirStatusEl.textContent = message;
  dirStatusEl.dataset.ok = String(ok);
}

function showTtsStatus(message: string, ok: boolean): void {
  ttsStatusEl.textContent = message;
  ttsStatusEl.dataset.ok = String(ok);
}

function setBusy(busy: boolean): void {
  synthesizeBtn.disabled = busy;
  saveAsBtn.disabled = busy;
  chooseDirBtn.disabled = busy;
  resetDirBtn.disabled = busy;
  synthesizeBtn.textContent = busy ? "Synthesizing..." : "Synthesize";
  progressEl.hidden = !busy;
}

let doneFlashTimer: number | undefined;

function flashDone(): void {
  synthesizeBtn.textContent = "Done";
  if (doneFlashTimer !== undefined) {
    window.clearTimeout(doneFlashTimer);
  }
  doneFlashTimer = window.setTimeout(() => {
    synthesizeBtn.textContent = "Synthesize";
  }, 1500);
}

let estimateTimer: number | undefined;
let lastProcessEstimate: number | null = null;

function scheduleEstimate(): void {
  charCount.textContent = String(textArea.value.length);
  if (estimateTimer !== undefined) {
    window.clearTimeout(estimateTimer);
  }
  estimateTimer = window.setTimeout(async () => {
    const text = textArea.value.trim();
    if (!text) {
      estimateEl.textContent = "0.0";
      processEstimateEl.textContent = "0.0";
      lastProcessEstimate = null;
      return;
    }
    try {
      const estimate = await invoke<DurationEstimate>("estimate_duration", { text });
      estimateEl.textContent = estimate.estimated_audio_secs.toFixed(1);
      processEstimateEl.textContent = estimate.estimated_process_secs.toFixed(1);
      lastProcessEstimate = estimate.estimated_process_secs;
    } catch {
      estimateEl.textContent = "0.0";
      processEstimateEl.textContent = "0.0";
      lastProcessEstimate = null;
    }
  }, 250);
}

textArea.addEventListener("input", scheduleEstimate);
scheduleEstimate();

pasteBtn.addEventListener("click", async () => {
  try {
    const text = await navigator.clipboard.readText();
    if (!text) {
      showTtsStatus("Clipboard is empty.", false);
      return;
    }
    textArea.value = text;
    scheduleEstimate();
    showTtsStatus("Text pasted from clipboard.", true);
  } catch {
    showTtsStatus("Clipboard read failed. Select the text and press Ctrl+V instead.", false);
  }
});

loadBtn.addEventListener("click", () => {
  fileInput.click();
});

clearBtn.addEventListener("click", () => {
  textArea.value = "";
  charCount.textContent = "0";
  estimateEl.textContent = "0.0";
  processEstimateEl.textContent = "0.0";
  lastProcessEstimate = null;
  resultBox.hidden = true;
  showTtsStatus("Text cleared.", true);
  textArea.focus();
});

fileInput.addEventListener("change", () => {
  const file = fileInput.files?.[0];
  fileInput.value = "";
  if (!file) return;
  const reader = new FileReader();
  reader.onload = () => {
    const content = typeof reader.result === "string" ? reader.result : "";
    textArea.value = content;
    scheduleEstimate();
    showTtsStatus(`Loaded ${file.name}.`, true);
  };
  reader.onerror = () => {
    showTtsStatus("Failed to read the file.", false);
  };
  reader.readAsText(file);
});

// ---- Models folder picker ----

function renderVoiceList(voices: InstalledVoice[]): void {
  voiceSelect.replaceChildren();

  if (voices.length === 0) {
    const empty = document.createElement("option");
    empty.value = "";
    empty.textContent =
      "No voices found. Choose a folder with model files or read the Help section.";
    empty.disabled = true;
    empty.selected = true;
    voiceSelect.appendChild(empty);
    voiceActiveLabel.hidden = true;
    return;
  }

  const stillExists =
    selectedVoiceId !== null && voices.some((voice) => voice.id === selectedVoiceId);
  if (!stillExists) {
    selectedVoiceId = null;
  }

  const placeholder = document.createElement("option");
  placeholder.value = "";
  placeholder.textContent = "Select a voice...";
  placeholder.disabled = true;
  placeholder.selected = selectedVoiceId === null;
  voiceSelect.appendChild(placeholder);

  const byLanguage = new Map<string, InstalledVoice[]>();
  for (const voice of voices) {
    const language = voice.language || "Unknown";
    let group = byLanguage.get(language);
    if (!group) {
      group = [];
      byLanguage.set(language, group);
    }
    group.push(voice);
  }

  for (const [language, languageVoices] of byLanguage) {
    const optgroup = document.createElement("optgroup");
    optgroup.label = language;
    for (const voice of languageVoices) {
      const option = document.createElement("option");
      option.value = voice.id;
      option.textContent = voice.display_name;
      if (voice.id === selectedVoiceId) {
        option.selected = true;
      }
      optgroup.appendChild(option);
    }
    voiceSelect.appendChild(optgroup);
  }

  voiceActiveLabel.hidden = selectedVoiceId === null;
}

voiceSelect.addEventListener("change", () => {
  selectedVoiceId = voiceSelect.value || null;
  voiceActiveLabel.hidden = selectedVoiceId === null;
});

async function refreshVoices(): Promise<void> {
  try {
    const voices = await invoke<InstalledVoice[]>("list_installed_voices");
    renderVoiceList(voices);
  } catch (error) {
    voiceSelect.replaceChildren();
    const empty = document.createElement("option");
    empty.value = "";
    empty.textContent = `Voice list unavailable: ${String(error)}`;
    empty.disabled = true;
    empty.selected = true;
    voiceSelect.appendChild(empty);
    voiceActiveLabel.hidden = true;
  }
}

async function loadModelsDir(): Promise<void> {
  try {
    const dir = await invoke<string>("get_models_dir");
    modelsDirLabel.textContent = dir;
    modelsDirLabel.title = dir;
    showDirStatus("", true);
  } catch (error) {
    modelsDirLabel.textContent = "Unavailable";
    showDirStatus(`Could not read the models folder: ${String(error)}`, false);
  }
}

chooseDirBtn.addEventListener("click", async () => {
  let path: string | null;
  try {
    path = await open({
      directory: true,
      multiple: false,
      title: "Select your models folder",
    });
  } catch (error) {
    showDirStatus(`Folder dialog failed: ${String(error)}`, false);
    return;
  }
  if (path === null) return;
  showDirStatus("Using selected folder...", true);
  try {
    await invoke("set_models_dir", { path });
    await loadModelsDir();
    await refreshVoices();
    showDirStatus("Models folder updated.", true);
  } catch (error) {
    showDirStatus(`Could not use that folder: ${String(error)}`, false);
  }
});

resetDirBtn.addEventListener("click", async () => {
  showDirStatus("Restoring bundled models...", true);
  try {
    await invoke("reset_models_dir");
    await loadModelsDir();
    await refreshVoices();
    showDirStatus("Bundled models restored.", true);
  } catch (error) {
    showDirStatus(`Could not restore bundled models: ${String(error)}`, false);
  }
});

// ---- Voices ----

async function loadVoices(): Promise<void> {
  await loadModelsDir();
  await refreshVoices();
}

void loadVoices();

async function synthesizeTo(outPath: string | null): Promise<void> {
  const text = textArea.value.trim();
  if (!text) {
    showTtsStatus("Enter some text to synthesize.", false);
    return;
  }
  setBusy(true);
  const processSecs =
    lastProcessEstimate !== null
      ? lastProcessEstimate.toFixed(1)
      : (Array.from(text).length * 0.06 * 0.06).toFixed(1);
  showTtsStatus(`Synthesizing... (approx. ${processSecs} s on this PC)`, true);
  let succeeded = false;
  try {
    const args: Record<string, unknown> = { text, outPath };
    if (selectedVoiceId !== null) {
      args.voiceId = selectedVoiceId;
    }
    const result = await invoke<SynthesisResult>("synthesize", args);
    resultDuration.textContent = `${result.audio_secs.toFixed(2)} s`;
    resultPath.textContent = result.wav_path;
    resultBox.hidden = false;
    showTtsStatus(`Done — audio ready (${result.audio_secs.toFixed(2)} s).`, true);
    succeeded = true;
  } catch (error) {
    resultBox.hidden = true;
    showTtsStatus(`Synthesis failed: ${String(error)}`, false);
  } finally {
    setBusy(false);
    if (succeeded) {
      flashDone();
    }
  }
}

synthesizeBtn.addEventListener("click", () => {
  void synthesizeTo(null);
});

saveAsBtn.addEventListener("click", async () => {
  if (synthesizeBtn.disabled) return;
  const text = textArea.value.trim();
  if (!text) {
    showTtsStatus("Enter some text to synthesize.", false);
    return;
  }
  let path: string | null;
  try {
    path = await save({
      title: "Save synthesized audio",
      defaultPath: `piper-tts-${Date.now()}.wav`,
      filters: [{ name: "WAV audio", extensions: ["wav"] }],
    });
  } catch (error) {
    showTtsStatus(`Save dialog failed: ${String(error)}`, false);
    return;
  }
  if (path === null) {
    showTtsStatus("Save cancelled.", false);
    return;
  }
  await synthesizeTo(path);
});