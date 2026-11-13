import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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

interface SynthesisProgress {
  done: number;
  total: number;
  percent: number;
}

interface Mp3Progress {
  token: string;
  percent: number;
}

interface InstalledVoice {
  id: string;
  language: string;
  display_name: string;
  size_bytes: number;
  quality: string;
}

type QueueStatus = "pending" | "working" | "done" | "error";

interface QueueItem {
  id: string;
  title: string;
  output_name: string;
  pdf_path: string | null;
  text: string | null;
  status: QueueStatus;
  error: string | null;
  wav_path: string | null;
  mp3_path: string | null;
  audio_secs: number | null;
}

interface QueueSnapshot {
  running: boolean;
  finished: boolean;
  completed: number;
  total: number;
  items: QueueItem[];
}

const app = document.querySelector<HTMLDivElement>("#app")!;

app.innerHTML = `
  <main class="shell">
    <header class="app-header">
      <h1>Piper TTS Reader</h1>
      <p class="muted">
        Synthesize text with your locally installed Piper voices.
      </p>
    </header>

    <div class="layout">
      <aside class="panel queue-panel">
        <h2>Cola de archivos</h2>
        <div class="queue-toolbar">
          <button id="btn-add-chapters" type="button">Agregar archivos...</button>
          <button id="btn-queue-toggle" type="button" class="primary" disabled>Iniciar cola</button>
        </div>
        <p class="queue-summary" id="queue-summary"></p>
        <div class="queue-overall" id="queue-overall" hidden>
          <div class="progress queue-overall-bar">
            <div class="progress-fill" id="queue-overall-fill"></div>
          </div>
          <p class="progress-label" id="queue-overall-label"></p>
        </div>
        <ul class="queue-list" id="queue-list"></ul>
        <p id="queue-status" class="status" role="status"></p>
      </aside>

      <div class="content-column">
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
            <div class="progress-fill indeterminate"></div>
          </div>
          <p class="progress-label" id="tts-progress-label" hidden></p>

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

          <div class="folder-picker">
            <div class="folder-picker-row">
              <label for="btn-choose-output">Output folder</label>
              <span
                id="output-dir-label"
                class="models-dir-label"
                title=""
              >Loading...</span>
            </div>
            <div class="folder-picker-actions">
              <button id="btn-choose-output" type="button">Choose output folder...</button>
              <button id="btn-reset-output" type="button" class="secondary">Use default output</button>
            </div>
            <p id="output-status" class="status" role="status"></p>
          </div>

          <label class="checkbox-row" for="mp3-auto-check">
            <input type="checkbox" id="mp3-auto-check" />
            <span>Convert finished files to MP3 (128 kbps)</span>
          </label>

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
            <button id="btn-load" type="button">Load file...</button>
            <button id="btn-clear" type="button">Clear</button>
            <button id="btn-synthesize" type="button" class="primary">Synthesize</button>
            <button id="btn-cancel" type="button" class="secondary" disabled>Cancel</button>
            <button id="btn-save-as" type="button">Save WAV as...</button>
          </div>

          <p id="tts-status" class="status" role="status"></p>
          <dl class="result" id="tts-result" hidden>
            <dt>Audio</dt>
            <dd id="result-duration"></dd>
            <dt>File</dt>
            <dd id="result-path"></dd>
          </dl>
          <div class="result-actions" id="tts-result-actions" hidden>
            <button id="btn-mp3" type="button">Convert to MP3...</button>
          </div>
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
          <h3>MP3 conversion</h3>
          <p>
            Finished WAV files can be converted to MP3 (128 kbps) right inside the app: use
            "Convert to MP3..." after direct synthesis, or the MP3 button on a finished queue item.
            The encoder is built into the application — no extra software or internet connection is
            needed.
          </p>
          <p>
            Tick "Convert finished files to MP3" to convert automatically: every queue item and
            every direct synthesis is encoded right after the WAV is written, saved next to it with
            the same name. Queue items show a "✓ MP3" badge when the MP3 was produced.
          </p>
          <h3>Output folder</h3>
          <p>
            Choose an "Output folder" to send every queue item (PDFs and pasted text) there, once,
            for the whole session. The choice is remembered for the next time you open the app; use
            "Use default output" to go back to saving PDFs next to their PDF and text in the app's
            output folder.
          </p>
        </section>
      </div>
    </div>
  </main>
`;

// ---- Conversion view ----

const textArea = document.querySelector<HTMLTextAreaElement>("#tts-text")!;
const charCount = document.querySelector<HTMLSpanElement>("#tts-chars")!;
const estimateEl = document.querySelector<HTMLSpanElement>("#tts-estimate")!;
const processEstimateEl = document.querySelector<HTMLSpanElement>("#tts-estimate-process")!;
const progressEl = document.querySelector<HTMLDivElement>("#tts-progress")!;
const progressFillEl = document.querySelector<HTMLDivElement>("#tts-progress .progress-fill")!;
const progressLabelEl = document.querySelector<HTMLParagraphElement>("#tts-progress-label")!;
const ttsStatusEl = document.querySelector<HTMLParagraphElement>("#tts-status")!;
const resultBox = document.querySelector<HTMLDListElement>("#tts-result")!;
const resultActions = document.querySelector<HTMLDivElement>("#tts-result-actions")!;
const resultDuration = document.querySelector<HTMLElement>("#result-duration")!;
const resultPath = document.querySelector<HTMLElement>("#result-path")!;
const mp3Btn = document.querySelector<HTMLButtonElement>("#btn-mp3")!;
const pasteBtn = document.querySelector<HTMLButtonElement>("#btn-paste")!;
const loadBtn = document.querySelector<HTMLButtonElement>("#btn-load")!;
const clearBtn = document.querySelector<HTMLButtonElement>("#btn-clear")!;
const synthesizeBtn = document.querySelector<HTMLButtonElement>("#btn-synthesize")!;
const cancelBtn = document.querySelector<HTMLButtonElement>("#btn-cancel")!;
const saveAsBtn = document.querySelector<HTMLButtonElement>("#btn-save-as")!;
const chooseDirBtn = document.querySelector<HTMLButtonElement>("#btn-choose-dir")!;
const resetDirBtn = document.querySelector<HTMLButtonElement>("#btn-reset-dir")!;
const modelsDirLabel = document.querySelector<HTMLSpanElement>("#models-dir-label")!;
const dirStatusEl = document.querySelector<HTMLParagraphElement>("#dir-status")!;
const chooseOutputBtn = document.querySelector<HTMLButtonElement>("#btn-choose-output")!;
const resetOutputBtn = document.querySelector<HTMLButtonElement>("#btn-reset-output")!;
const outputDirLabel = document.querySelector<HTMLSpanElement>("#output-dir-label")!;
const outputStatusEl = document.querySelector<HTMLParagraphElement>("#output-status")!;
const mp3AutoCheck = document.querySelector<HTMLInputElement>("#mp3-auto-check")!;
const voiceSelect = document.querySelector<HTMLSelectElement>("#voice-select")!;
const voiceActiveLabel = document.querySelector<HTMLSpanElement>("#voice-active-label")!;

let selectedVoiceId: string | null = null;

// Active output directory (global output folder when set, default otherwise).
// Cached at startup and refreshed whenever the user changes the folder, so the
// direct "Save WAV as..." dialog opens there by default.
let outputDir: string | null = null;

// Whether finished files are auto-converted to MP3 (checkbox). Applies to both
// the queue and the direct convert panel; session-only.
let mp3AutoConvert = false;

// WAV produced by the last direct synthesis ("Save WAV as..."); the MP3 button
// converts this file.
let lastWavPath: string | null = null;

// In-flight MP3 conversions keyed by their progress token: "direct" for the
// convert panel, or the queue item id for queue conversions. The value is the
// latest progress percent (0..100).
const convertingMp3 = new Map<string, number>();

function showDirStatus(message: string, ok: boolean): void {
  dirStatusEl.textContent = message;
  dirStatusEl.dataset.ok = String(ok);
}

function showOutputStatus(message: string, ok: boolean): void {
  outputStatusEl.textContent = message;
  outputStatusEl.dataset.ok = String(ok);
}

async function loadOutputDir(): Promise<void> {
  try {
    outputDir = await invoke<string>("get_output_dir");
    outputDirLabel.textContent = outputDir;
    outputDirLabel.title = outputDir;
    showOutputStatus("", true);
  } catch (error) {
    outputDirLabel.textContent = "Unavailable";
    showOutputStatus(`Could not read the output folder: ${String(error)}`, false);
  }
}

function showTtsStatus(message: string, ok: boolean): void {
  ttsStatusEl.textContent = message;
  ttsStatusEl.dataset.ok = String(ok);
}

function setBusy(busy: boolean): void {
  synthesizeBtn.disabled = busy;
  saveAsBtn.disabled = busy;
  cancelBtn.disabled = !busy;
  chooseDirBtn.disabled = busy;
  resetDirBtn.disabled = busy;
  chooseOutputBtn.disabled = busy;
  resetOutputBtn.disabled = busy;
  progressEl.hidden = !busy;
  if (busy) {
    // Start indeterminate; the first synthesis-progress event switches the
    // bar to determinate with a real percentage.
    progressFillEl.classList.add("indeterminate");
    progressFillEl.style.width = "";
    progressLabelEl.hidden = true;
  } else {
    progressLabelEl.hidden = true;
  }
}

// Real per-sentence progress from the backend: fills the bar and shows which
// sentence is being synthesized.
void listen<SynthesisProgress>("synthesis-progress", (event) => {
  const progress = event.payload;
  progressFillEl.classList.remove("indeterminate");
  progressFillEl.style.width = `${progress.percent}%`;
  progressLabelEl.hidden = false;
  progressLabelEl.textContent =
    progress.done === 0
      ? `Synthesizing ${progress.total} sentence${progress.total === 1 ? "" : "s"}...`
      : `Sentence ${progress.done} of ${progress.total} — ${Math.round(progress.percent)}%`;
});

// MP3 conversion progress: token "direct" drives the convert panel's bar,
// other tokens (queue item ids) drive their item's bar.
void listen<Mp3Progress>("mp3-progress", (event) => {
  const { token, percent } = event.payload;
  convertingMp3.set(token, percent);
  if (token === "direct") {
    progressFillEl.classList.remove("indeterminate");
    progressFillEl.style.width = `${percent}%`;
    progressLabelEl.hidden = false;
    progressLabelEl.textContent = `Converting to MP3... ${Math.round(percent)}%`;
  } else {
    updateItemProgress(token, {
      item_id: token,
      done: Math.round(percent),
      total: 100,
      percent,
    });
  }
});

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

loadBtn.addEventListener("click", async () => {
  let path: string | null;
  try {
    path = await open({
      multiple: false,
      title: "Load text or PDF",
      filters: [
        { name: "Text or PDF", extensions: ["txt", "pdf"] },
      ],
    });
  } catch (error) {
    showTtsStatus(`Open dialog failed: ${String(error)}`, false);
    return;
  }
  if (path === null) return;
  try {
    const lower = path.toLowerCase();
    let content: string;
    if (lower.endsWith(".pdf")) {
      content = await invoke<string>("extract_pdf_text", { path });
    } else {
      content = await invoke<string>("read_text_file", { path });
    }
    textArea.value = content;
    scheduleEstimate();
    showTtsStatus(`Loaded ${path.split(/[\\/]/).pop()}.`, true);
  } catch (error) {
    showTtsStatus(`Could not load file: ${String(error)}`, false);
  }
});

clearBtn.addEventListener("click", () => {
  textArea.value = "";
  charCount.textContent = "0";
  estimateEl.textContent = "0.0";
  processEstimateEl.textContent = "0.0";
  lastProcessEstimate = null;
  resultBox.hidden = true;
  resultActions.hidden = true;
  lastWavPath = null;
  showTtsStatus("Text cleared.", true);
  textArea.focus();
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

// ---- Global output folder ----
//
// Picking a folder here makes EVERY queue item (PDFs and pasted text) write
// its WAV into it, and the direct "Save WAV as..." dialog opens there by
// default. The choice is persisted across sessions (settings file).

chooseOutputBtn.addEventListener("click", async () => {
  let path: string | null;
  try {
    path = await open({
      directory: true,
      multiple: false,
      title: "Select your output folder",
    });
  } catch (error) {
    showOutputStatus(`Folder dialog failed: ${String(error)}`, false);
    return;
  }
  if (path === null) return;
  showOutputStatus("Using selected folder...", true);
  try {
    await invoke("set_output_dir", { path });
    await loadOutputDir();
    showOutputStatus("Output folder updated. All finished files are saved here.", true);
  } catch (error) {
    showOutputStatus(`Could not use that folder: ${String(error)}`, false);
  }
});

resetOutputBtn.addEventListener("click", async () => {
  showOutputStatus("Restoring default output...", true);
  try {
    await invoke("reset_output_dir");
    await loadOutputDir();
    showOutputStatus("Default output restored.", true);
  } catch (error) {
    showOutputStatus(`Could not restore the default output: ${String(error)}`, false);
  }
});

// ---- MP3 auto-conversion checkbox ----
//
// When checked, every finished synthesis (queue items and the direct panel)
// is automatically converted to MP3 (128 kbps) right after the WAV is written.

mp3AutoCheck.addEventListener("change", () => {
  mp3AutoConvert = mp3AutoCheck.checked;
  void invoke("set_mp3_auto_convert", { enabled: mp3AutoConvert }).catch((error) => {
    mp3AutoCheck.checked = !mp3AutoConvert;
    mp3AutoConvert = !mp3AutoConvert;
    showTtsStatus(`Could not update MP3 auto-conversion: ${String(error)}`, false);
  });
});

async function loadMp3AutoConvert(): Promise<void> {
  try {
    mp3AutoConvert = await invoke<boolean>("get_mp3_auto_convert");
    mp3AutoCheck.checked = mp3AutoConvert;
  } catch (error) {
    mp3AutoCheck.disabled = true;
    showTtsStatus(`Could not read MP3 auto-conversion: ${String(error)}`, false);
  }
}

// ---- Voices ----

async function loadVoices(): Promise<void> {
  await loadModelsDir();
  await refreshVoices();
}

void loadVoices();
void loadOutputDir();
void loadMp3AutoConvert();

// ---- Chapter queue ----

const queueList = document.querySelector<HTMLUListElement>("#queue-list")!;
const queueSummary = document.querySelector<HTMLParagraphElement>("#queue-summary")!;
const queueStatusEl = document.querySelector<HTMLParagraphElement>("#queue-status")!;
const addChaptersBtn = document.querySelector<HTMLButtonElement>("#btn-add-chapters")!;
const queueToggleBtn = document.querySelector<HTMLButtonElement>("#btn-queue-toggle")!;

const queueOverall = document.querySelector<HTMLDivElement>("#queue-overall")!;
const queueOverallFill = document.querySelector<HTMLDivElement>("#queue-overall-fill")!;
const queueOverallLabel = document.querySelector<HTMLParagraphElement>("#queue-overall-label")!;

interface QueueProgress {
  item_id: string;
  done: number;
  total: number;
  percent: number;
}

let queueSnapshot: QueueSnapshot | null = null;

// Per-item progress received from the `queue-progress` event, keyed by item id.
const queueProgressMap = new Map<string, QueueProgress>();

function showQueueStatus(message: string, ok: boolean): void {
  queueStatusEl.textContent = message;
  queueStatusEl.dataset.ok = String(ok);
}

function queueBadgeLabel(item: QueueItem): string {
  switch (item.status) {
    case "pending":
      return "En cola";
    case "working": {
      // While the automatic post-synthesis conversion runs, the item is still
      // Working; mirror the MP3 progress on the badge instead of the last
      // sentence number.
      const mp3Percent = convertingMp3.get(item.id);
      if (mp3Percent !== undefined) {
        return `MP3 ${Math.round(mp3Percent)}%`;
      }
      const progress = queueProgressMap.get(item.id);
      return progress !== undefined
        ? `Oración ${progress.done} de ${progress.total} (${Math.round(progress.percent)}%)`
        : "Trabajando...";
    }
    case "done": {
      const mp3Percent = convertingMp3.get(item.id);
      if (mp3Percent !== undefined) {
        return `MP3 ${Math.round(mp3Percent)}%`;
      }
      if (item.mp3_path !== null) {
        return "✓ MP3";
      }
      if (item.error !== null) {
        return "Listo · MP3 falló";
      }
      return item.audio_secs != null ? `Listo (${item.audio_secs.toFixed(1)} s)` : "Listo";
    }
    case "error":
      return `Error: ${item.error ?? "desconocido"}`;
  }
}

function renderQueueItem(item: QueueItem, running: boolean): HTMLLIElement {
  const li = document.createElement("li");
  li.className = `queue-item ${item.status}`;
  li.dataset.itemId = item.id;

  const top = document.createElement("div");
  top.className = "queue-item-top";

  const title = document.createElement("span");
  title.className = "queue-item-title";
  title.textContent = item.title;
  title.title = item.pdf_path ?? "";
  top.appendChild(title);

  const badge = document.createElement("span");
  badge.className = "queue-badge";
  badge.textContent = queueBadgeLabel(item);
  badge.title =
    (item.status === "error" && item.error !== null) ||
    (item.status === "done" && item.error !== null)
      ? item.error
      : item.mp3_path ?? "";
  top.appendChild(badge);

  const removeBtn = document.createElement("button");
  removeBtn.type = "button";
  removeBtn.className = "queue-remove";
  removeBtn.textContent = "Quitar";
  removeBtn.disabled = running || item.status === "working";
  removeBtn.addEventListener("click", () => {
    void removeChapter(item.id);
  });
  top.appendChild(removeBtn);

  // Finished items can be exported as MP3 right from the queue. The button is
  // disabled while a conversion for this item is already running.
  if (item.status === "done" && item.wav_path !== null) {
    const mp3Button = document.createElement("button");
    mp3Button.type = "button";
    mp3Button.className = "queue-mp3";
    mp3Button.textContent = "MP3";
    mp3Button.title = item.wav_path;
    mp3Button.disabled = running || convertingMp3.has(item.id);
    mp3Button.addEventListener("click", () => {
      void convertQueueItemToMp3(item);
    });
    top.appendChild(mp3Button);
  }

  li.appendChild(top);

  // Editable output file name (without the .wav extension).
  const nameRow = document.createElement("div");
  nameRow.className = "queue-name-row";

  const nameLabel = document.createElement("label");
  nameLabel.className = "queue-name-label";
  nameLabel.textContent = "Nombre:";

  const nameInput = document.createElement("input");
  nameInput.type = "text";
  nameInput.className = "queue-name-input";
  nameInput.value = item.output_name;
  nameInput.disabled = running;
  nameInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter") nameInput.blur();
  });
  nameInput.addEventListener("change", () => {
    const value = nameInput.value.trim();
    if (value === item.output_name) return;
    void invoke("queue_set_output_name", { id: item.id, name: value })
      .then(() => refreshQueueState())
      .catch((error) => {
        showQueueStatus(`No se pudo cambiar el nombre: ${String(error)}`, false);
        void refreshQueueState();
      });
  });

  const nameExt = document.createElement("span");
  nameExt.className = "queue-name-ext";
  nameExt.textContent = ".wav";

  nameRow.appendChild(nameLabel);
  nameRow.appendChild(nameInput);
  nameRow.appendChild(nameExt);
  li.appendChild(nameRow);

  // Per-item progress: the working item shows its synthesis bar, and a
  // finished item shows its bar while an MP3 conversion runs.
  const progressBox = document.createElement("div");
  progressBox.className = "queue-item-progress";
  const bar = document.createElement("div");
  bar.className = "progress queue-item-bar";
  const fill = document.createElement("div");
  fill.className = "progress-fill queue-item-fill";
  bar.appendChild(fill);
  const progressLabel = document.createElement("p");
  progressLabel.className = "progress-label queue-item-progress-label";
  progressBox.appendChild(bar);
  progressBox.appendChild(progressLabel);

  if (item.status === "working") {
    const mp3Percent = convertingMp3.get(item.id);
    if (mp3Percent !== undefined) {
      fill.style.width = `${mp3Percent}%`;
      progressLabel.textContent = `MP3 ${Math.round(mp3Percent)}%`;
    } else {
      const stored = queueProgressMap.get(item.id);
      if (stored !== undefined) {
        fill.style.width = `${stored.percent}%`;
        progressLabel.textContent = `Oración ${stored.done} de ${stored.total} (${Math.round(stored.percent)}%)`;
      }
    }
  } else if (item.status === "done" && convertingMp3.has(item.id)) {
    // The item's own bar mirrors its MP3 conversion progress.
    const percent = convertingMp3.get(item.id)!;
    fill.style.width = `${percent}%`;
    progressLabel.textContent = `MP3 ${Math.round(percent)}%`;
  } else {
    progressBox.hidden = true;
  }
  li.appendChild(progressBox);

  return li;
}

function renderQueue(): void {
  if (queueSnapshot === null) return;

  // Drop conversion markers for items that already finished their MP3: the
  // snapshot now carries `mp3_path` and the badge must show "✓ MP3", not a
  // stale percentage.
  for (const item of queueSnapshot.items) {
    if (item.status === "done" && item.mp3_path !== null) {
      convertingMp3.delete(item.id);
    }
  }

  queueList.replaceChildren();

  // Finished items stay visible so the user can export them as MP3 or remove
  // them (they are no longer auto-hidden after a few seconds).
  const visibleItems = queueSnapshot.items;

  if (visibleItems.length === 0) {
    const empty = document.createElement("li");
    empty.className = "queue-empty";
    empty.textContent = "Sin archivos. Agrega PDFs o texto para comenzar.";
    queueList.appendChild(empty);
  }
  for (const item of visibleItems) {
    queueList.appendChild(renderQueueItem(item, queueSnapshot.running));
  }

  queueSummary.textContent =
    queueSnapshot.total > 0 ? `${queueSnapshot.completed}/${queueSnapshot.total}` : "";

  if (queueSnapshot.total > 0) {
    queueOverall.hidden = false;
    const percent = (queueSnapshot.completed / queueSnapshot.total) * 100;
    queueOverallFill.style.width = `${percent}%`;
    queueOverallLabel.textContent = `${queueSnapshot.completed}/${queueSnapshot.total} archivos (${Math.round(percent)}%)`;
  } else {
    queueOverall.hidden = true;
  }

  if (queueSnapshot.finished) {
    const hasPending = queueSnapshot.items.some((item) => item.status === "pending");
    showQueueStatus(hasPending ? "Cola detenida." : "Cola terminada.", !hasPending);
  }

  const hasPending = queueSnapshot.items.some((item) => item.status === "pending");
  queueToggleBtn.disabled = !queueSnapshot.running && !hasPending;
  queueToggleBtn.textContent = queueSnapshot.running ? "Detener" : "Iniciar cola";
  addChaptersBtn.disabled = queueSnapshot.running;
  chooseOutputBtn.disabled = queueSnapshot.running;
  resetOutputBtn.disabled = queueSnapshot.running;

  // While the queue runs the voice is locked for the whole run, and direct
  // synthesis is disabled so nothing races the queue's piper jobs. The direct
  // MP3 button is also disabled so it cannot fight the queue's synthesis bar.
  voiceSelect.disabled = queueSnapshot.running;
  synthesizeBtn.disabled = queueSnapshot.running;
  saveAsBtn.disabled = queueSnapshot.running;
  mp3Btn.disabled = queueSnapshot.running || convertingMp3.has("direct");
}

function updateItemProgress(itemId: string, progress: QueueProgress): void {
  const itemEl = queueList.querySelector<HTMLElement>(`[data-item-id="${itemId}"]`);
  if (itemEl === null) return;
  const progressBox = itemEl.querySelector<HTMLElement>(".queue-item-progress");
  const fill = itemEl.querySelector<HTMLElement>(".queue-item-fill");
  const label = itemEl.querySelector<HTMLElement>(".queue-item-progress-label");
  const badge = itemEl.querySelector<HTMLElement>(".queue-badge");
  const isMp3 = convertingMp3.has(itemId);
  if (progressBox !== null) progressBox.hidden = false;
  if (fill !== null) fill.style.width = `${progress.percent}%`;
  const text = isMp3
    ? `MP3 ${Math.round(progress.percent)}%`
    : `Oración ${progress.done} de ${progress.total} (${Math.round(progress.percent)}%)`;
  if (label !== null) label.textContent = text;
  if (badge !== null) badge.textContent = text;
}

async function refreshQueueState(): Promise<void> {
  try {
    queueSnapshot = await invoke<QueueSnapshot>("queue_state");
    renderQueue();
  } catch (error) {
    showQueueStatus(`No se pudo leer la cola: ${String(error)}`, false);
  }
}

void listen<QueueSnapshot>("queue-updated", (event) => {
  queueSnapshot = event.payload;
  renderQueue();
});

void listen<QueueProgress>("queue-progress", (event) => {
  const progress = event.payload;
  queueProgressMap.set(progress.item_id, progress);
  updateItemProgress(progress.item_id, progress);
});

addChaptersBtn.addEventListener("click", async () => {
  let paths: string[] | null;
  try {
    paths = await open({
      multiple: true,
      title: "Agregar archivos (PDF)",
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
  } catch (error) {
    showQueueStatus(`Diálogo de archivos falló: ${String(error)}`, false);
    return;
  }
  if (paths === null || paths.length === 0) return;
  try {
    await invoke<QueueItem[]>("queue_add_documents", { paths });
    await refreshQueueState();
    showQueueStatus(
      paths.length === 1 ? "Archivo agregado." : `${paths.length} archivos agregados.`,
      true,
    );
  } catch (error) {
    showQueueStatus(`No se pudieron agregar los archivos: ${String(error)}`, false);
  }
});

queueToggleBtn.addEventListener("click", async () => {
  if (queueSnapshot !== null && queueSnapshot.running) {
    try {
      await invoke("queue_stop");
      showQueueStatus("Deteniendo cola...", true);
    } catch (error) {
      showQueueStatus(`No se pudo detener la cola: ${String(error)}`, false);
    }
    return;
  }
  try {
    const args: Record<string, unknown> = {};
    if (selectedVoiceId !== null) {
      args.voiceId = selectedVoiceId;
    }
    await invoke("queue_start", args);
    showQueueStatus("Cola iniciada.", true);
  } catch (error) {
    showQueueStatus(`No se pudo iniciar la cola: ${String(error)}`, false);
  }
});

async function removeChapter(id: string): Promise<void> {
  try {
    await invoke("queue_remove", { id });
    await refreshQueueState();
  } catch (error) {
    showQueueStatus(`No se pudo quitar el archivo: ${String(error)}`, false);
  }
}

void refreshQueueState();

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
  try {
    const args: Record<string, unknown> = { text, outPath };
    if (selectedVoiceId !== null) {
      args.voiceId = selectedVoiceId;
    }
    const result = await invoke<SynthesisResult>("synthesize", args);
    resultDuration.textContent = `${result.audio_secs.toFixed(2)} s`;
    resultPath.textContent = result.wav_path;
    resultBox.hidden = false;
    resultActions.hidden = false;
    lastWavPath = result.wav_path;
    mp3Btn.disabled = false;
    showTtsStatus(`Done — audio ready (${result.audio_secs.toFixed(2)} s).`, true);

    // Auto-convert when the checkbox is on: same folder, same base name.
    if (mp3AutoConvert && result.wav_path.toLowerCase().endsWith(".wav")) {
      const mp3Path = result.wav_path.replace(/\.wav$/i, ".mp3");
      await convertToMp3(result.wav_path, mp3Path, "direct");
    }
  } catch (error) {
    resultBox.hidden = true;
    resultActions.hidden = true;
    const message = String(error);
    if (message.toLowerCase().includes("cancelled")) {
      showTtsStatus("Synthesis cancelled.", false);
    } else {
      showTtsStatus(`Synthesis failed: ${message}`, false);
    }
  } finally {
    setBusy(false);
  }
}

async function addTextToQueue(): Promise<void> {
  const text = textArea.value.trim();
  if (!text) {
    showTtsStatus("Escribe o pega texto para agregarlo a la cola.", false);
    return;
  }
  try {
    await invoke<QueueItem[]>("queue_add_text", { text });
    textArea.value = "";
    scheduleEstimate();
    resultBox.hidden = true;
    resultActions.hidden = true;
    lastWavPath = null;
    await refreshQueueState();
    showTtsStatus("Texto agregado a la cola de archivos.", true);
  } catch (error) {
    showTtsStatus(`No se pudo agregar el texto a la cola: ${String(error)}`, false);
  }
}

synthesizeBtn.addEventListener("click", () => {
  void addTextToQueue();
});

cancelBtn.addEventListener("click", async () => {
  try {
    await invoke("cancel_synthesis");
    showTtsStatus("Cancelling...", true);
  } catch (error) {
    showTtsStatus(`Cancel failed: ${String(error)}`, false);
  }
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
    const outDir = outputDir ?? (await invoke<string>("get_output_dir"));
    const sep = outDir.includes("\\") ? "\\" : "/";
    const defaultPath = `${outDir.replace(/[\\/]+$/, "")}${sep}piper-tts-${Date.now()}.wav`;
    path = await save({
      title: "Save synthesized audio",
      defaultPath,
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

// ---- MP3 conversion ----

// Convert the given WAV to MP3 through the embedded encoder. `token` routes
// progress events: "direct" uses the convert panel's bar, a queue item id uses
// that item's own bar.
async function convertToMp3(wavPath: string, mp3Path: string, token: string): Promise<void> {
  if (convertingMp3.has(token)) return;
  convertingMp3.set(token, 0);
  const isDirect = token === "direct";

  if (isDirect) {
    mp3Btn.disabled = true;
    progressEl.hidden = false;
    progressFillEl.classList.add("indeterminate");
    progressFillEl.style.width = "";
    progressLabelEl.hidden = false;
    progressLabelEl.textContent = "Converting to MP3...";
    showTtsStatus("Converting to MP3... (128 kbps)", true);
  } else {
    renderQueue();
  }

  try {
    await invoke("convert_wav_to_mp3", { wavPath, mp3Path, token });
    convertingMp3.delete(token);
    if (isDirect) {
      progressEl.hidden = true;
      progressLabelEl.hidden = true;
      mp3Btn.disabled = false;
      showTtsStatus(`MP3 ready: ${mp3Path}`, true);
    } else {
      showQueueStatus(`MP3 ready: ${mp3Path}`, true);
      await refreshQueueState();
    }
  } catch (error) {
    convertingMp3.delete(token);
    if (isDirect) {
      progressEl.hidden = true;
      progressLabelEl.hidden = true;
      mp3Btn.disabled = false;
      showTtsStatus(`MP3 conversion failed: ${String(error)}`, false);
    } else {
      showQueueStatus(`MP3 conversion failed: ${String(error)}`, false);
      await refreshQueueState();
    }
  }
}

// Convert a finished queue item's WAV. The save dialog defaults to the same
// folder/name as the WAV with the .mp3 extension.
async function convertQueueItemToMp3(item: QueueItem): Promise<void> {
  if (item.wav_path === null || convertingMp3.has(item.id)) return;
  let path: string | null;
  try {
    path = await save({
      title: "Save MP3 audio",
      defaultPath: item.wav_path.replace(/\.wav$/i, ".mp3"),
      filters: [{ name: "MP3 audio", extensions: ["mp3"] }],
    });
  } catch (error) {
    showQueueStatus(`Save dialog failed: ${String(error)}`, false);
    return;
  }
  if (path === null) {
    showQueueStatus("MP3 conversion cancelled.", false);
    return;
  }
  await convertToMp3(item.wav_path, path, item.id);
}

mp3Btn.addEventListener("click", async () => {
  if (mp3Btn.disabled || lastWavPath === null) return;
  let path: string | null;
  try {
    path = await save({
      title: "Save MP3 audio",
      defaultPath: lastWavPath.replace(/\.wav$/i, ".mp3"),
      filters: [{ name: "MP3 audio", extensions: ["mp3"] }],
    });
  } catch (error) {
    showTtsStatus(`Save dialog failed: ${String(error)}`, false);
    return;
  }
  if (path === null) {
    showTtsStatus("MP3 conversion cancelled.", false);
    return;
  }
  await convertToMp3(lastWavPath, path, "direct");
});