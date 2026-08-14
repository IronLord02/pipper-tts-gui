import { invoke } from "@tauri-apps/api/core";

type ProxyMode =
  | { mode: "none" }
  | { mode: "system" }
  | { mode: "manual"; host: string; port: number };

const app = document.querySelector<HTMLDivElement>("#app")!;

app.innerHTML = `
  <main class="shell">
    <h1>Piper TTS Reader</h1>
    <p class="muted">
      Frontend shell is up. The catalog browser, download controls, and
      conversion view arrive in later slices.
    </p>

    <section class="panel proxy-panel">
      <h2>Network / proxy</h2>
      <p class="hint">
        Choose how model downloads reach the network. Pick <em>manual</em> if
        your connection requires a proxy (for example a corporate proxy).
      </p>

      <label class="row">
        <input type="radio" name="proxy" value="none" checked />
        <span><strong>No proxy</strong> — direct connection</span>
      </label>

      <label class="row">
        <input type="radio" name="proxy" value="system" />
        <span><strong>System proxy</strong> — use the proxy configured on this
        computer / in the environment</span>
      </label>

      <label class="row">
        <input type="radio" name="proxy" value="manual" />
        <span><strong>Manual proxy</strong> — specify host and port</span>
      </label>

      <div class="manual-fields">
        <label>
          Proxy host / IP
          <input id="proxy-host" type="text" placeholder="e.g. 172.16.21.3" />
        </label>
        <label>
          Port
          <input id="proxy-port" type="number" min="1" max="65535" placeholder="e.g. 3128" />
        </label>
      </div>

      <button id="save-proxy" type="button">Save proxy setting</button>
      <p id="proxy-status" class="status" role="status"></p>
    </section>
  </main>
`;

const hostInput = document.querySelector<HTMLInputElement>("#proxy-host")!;
const portInput = document.querySelector<HTMLInputElement>("#proxy-port")!;
const statusEl = document.querySelector<HTMLParagraphElement>("#proxy-status")!;
const saveBtn = document.querySelector<HTMLButtonElement>("#save-proxy")!;
const manualFields = document.querySelector<HTMLDivElement>(".manual-fields")!;
const radios = document.querySelectorAll<HTMLInputElement>('input[name="proxy"]');

function showStatus(message: string, ok: boolean): void {
  statusEl.textContent = message;
  statusEl.dataset.ok = String(ok);
}

function readSelectedMode(): ProxyMode {
  const selected = [...radios].find((radio) => radio.checked)?.value ?? "none";
  if (selected === "manual") {
    return {
      mode: "manual",
      host: hostInput.value.trim(),
      port: Number(portInput.value),
    };
  }
  return { mode: selected as "none" | "system" };
}

function applyModeToForm(mode: ProxyMode): void {
  for (const radio of radios) {
    radio.checked = radio.value === mode.mode;
  }
  if (mode.mode === "manual") {
    hostInput.value = mode.host;
    portInput.value = String(mode.port);
  }
  manualFields.classList.toggle("enabled", mode.mode === "manual");
}

for (const radio of radios) {
  radio.addEventListener("change", () => {
    manualFields.classList.toggle("enabled", radio.value === "manual");
  });
}

saveBtn.addEventListener("click", async () => {
  const mode = readSelectedMode();
  if (mode.mode === "manual") {
    if (!mode.host) {
      showStatus("Enter a proxy host / IP.", false);
      return;
    }
    if (!Number.isInteger(mode.port) || mode.port < 1 || mode.port > 65535) {
      showStatus("Enter a valid port (1-65535).", false);
      return;
    }
  }
  try {
    await invoke("set_proxy", { mode });
    showStatus("Proxy setting saved.", true);
  } catch (error) {
    showStatus(`Failed to save: ${String(error)}`, false);
  }
});

// Load the current setting when the shell starts.
(async () => {
  try {
    const mode = await invoke<ProxyMode>("get_proxy");
    applyModeToForm(mode);
  } catch (error) {
    showStatus(`Failed to load proxy setting: ${String(error)}`, false);
  }
})();
