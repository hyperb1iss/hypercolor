//! Live preview page for browser-based effect visualization.

use axum::response::{Html, IntoResponse, Response};

/// `GET /preview` — serve a lightweight browser UI for live canvas preview.
pub async fn preview_page() -> Response {
    Html(PREVIEW_HTML).into_response()
}

const PREVIEW_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Hypercolor Live Preview</title>
  <style>
    :root {
      --bg: #071019;
      --surface: #0f1c29;
      --line: #1f3647;
      --text: #e5f3ff;
      --muted: #8fb0c4;
      --accent: #4ef2ff;
      --warn: #ffdd7d;
      --ok: #61f5a3;
      --err: #ff7d8a;
    }

    * { box-sizing: border-box; }
    body {
      margin: 0;
      font-family: "JetBrains Mono", "Fira Code", "SFMono-Regular", ui-monospace, monospace;
      background: radial-gradient(circle at 20% 0%, #12283a 0%, var(--bg) 55%);
      color: var(--text);
      min-height: 100vh;
      display: grid;
      place-items: center;
      padding: 24px;
    }

    .app {
      width: min(1100px, 100%);
      background: color-mix(in srgb, var(--surface) 92%, black);
      border: 1px solid var(--line);
      border-radius: 14px;
      overflow: hidden;
      box-shadow: 0 18px 50px rgba(0, 0, 0, 0.45);
    }

    .top {
      padding: 14px 16px;
      border-bottom: 1px solid var(--line);
      display: grid;
      gap: 10px;
    }

    .title {
      font-size: 14px;
      letter-spacing: 0.05em;
      color: var(--accent);
      text-transform: uppercase;
    }

    .controls {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      align-items: center;
    }

    select, button, input {
      background: #122232;
      border: 1px solid #204159;
      color: var(--text);
      border-radius: 8px;
      padding: 8px 10px;
      font: inherit;
      min-height: 36px;
    }

    button {
      cursor: pointer;
      transition: border-color 130ms ease, transform 130ms ease;
    }
    button:hover {
      border-color: var(--accent);
      transform: translateY(-1px);
    }
    button:disabled, select:disabled, input:disabled {
      opacity: 0.55;
      cursor: not-allowed;
      transform: none;
    }

    .toggle {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      font-size: 12px;
      color: var(--muted);
      user-select: none;
    }

    .toggle input {
      min-height: 0;
      width: 15px;
      height: 15px;
      padding: 0;
      margin: 0;
    }

    .meta {
      display: flex;
      flex-wrap: wrap;
      gap: 14px;
      font-size: 12px;
      color: var(--muted);
    }

    .hint {
      font-size: 12px;
      color: var(--muted);
      line-height: 1.4;
    }

    .hint code {
      padding: 1px 4px;
      border-radius: 5px;
      border: 1px solid #24465f;
      background: #08131d;
      color: var(--text);
    }

    .status-ok { color: var(--ok); }
    .status-warn { color: var(--warn); }
    .status-err { color: var(--err); }

    .stage {
      padding: 16px;
      display: grid;
      gap: 12px;
      justify-items: center;
      background:
        linear-gradient(to right, rgba(255,255,255,0.02) 1px, transparent 1px) 0 0 / 18px 18px,
        linear-gradient(to bottom, rgba(255,255,255,0.02) 1px, transparent 1px) 0 0 / 18px 18px,
        #081520;
    }

    canvas {
      width: min(960px, 100%);
      aspect-ratio: 16 / 10;
      border: 1px solid #24465f;
      border-radius: 10px;
      image-rendering: pixelated;
      background: #000;
    }

    .circular-mask {
      border-radius: 999px;
    }

    .log {
      width: min(960px, 100%);
      max-height: 140px;
      overflow: auto;
      border: 1px solid var(--line);
      border-radius: 8px;
      padding: 10px;
      background: #07131e;
      font-size: 12px;
      color: var(--muted);
      line-height: 1.45;
      white-space: pre-wrap;
    }
  </style>
</head>
<body>
  <main class="app">
    <header class="top">
      <div class="title">Hypercolor Live Preview</div>
      <div class="controls">
        <select id="previewMode">
          <option value="canvas">canvas</option>
          <option value="simulator">simulator</option>
        </select>
        <select id="simulatorSelect"></select>
        <select id="effectSelect"></select>
        <button id="applyBtn" type="button">Apply Effect</button>
        <button id="stopBtn" type="button">Stop</button>
        <input id="fpsInput" type="number" min="1" max="30" value="30" title="Canvas FPS" />
        <label class="toggle" for="showUnavailable">
          <input id="showUnavailable" type="checkbox" />
          show unavailable
        </label>
        <button id="reconnectBtn" type="button">Reconnect</button>
      </div>
      <div class="meta">
        <span>WS: <strong id="wsState" class="status-warn">connecting...</strong></span>
        <span>Preview: <strong id="previewModeLabel">canvas</strong></span>
        <span>Frames: <strong id="frameCount">0</strong></span>
        <span>Size: <strong id="canvasSize">-</strong></span>
        <span>Effect: <strong id="activeEffect">-</strong></span>
      </div>
      <div id="rendererHint" class="hint"></div>
    </header>
    <section class="stage">
      <canvas id="previewCanvas" width="320" height="200"></canvas>
      <div id="log" class="log"></div>
    </section>
  </main>

  <script>
    const stateEl = document.getElementById("wsState");
    const frameCountEl = document.getElementById("frameCount");
    const canvasSizeEl = document.getElementById("canvasSize");
    const activeEffectEl = document.getElementById("activeEffect");
    const modeEl = document.getElementById("previewMode");
    const modeLabelEl = document.getElementById("previewModeLabel");
    const simulatorEl = document.getElementById("simulatorSelect");
    const selectEl = document.getElementById("effectSelect");
    const applyBtn = document.getElementById("applyBtn");
    const fpsEl = document.getElementById("fpsInput");
    const showUnavailableEl = document.getElementById("showUnavailable");
    const rendererHintEl = document.getElementById("rendererHint");
    const logEl = document.getElementById("log");
    const canvas = document.getElementById("previewCanvas");
    const ctx = canvas.getContext("2d");
    const SERVO_RUN_HINT = "./scripts/run-preview-servo.sh";
    const urlState = new URLSearchParams(window.location.search);

    let ws = null;
    let frameCount = 0;
    let token = urlState.get("token") || "";
    let simulatorConfigs = [];
    let simulatorPollHandle = null;
    const requestedDisplay = urlState.get("display") || "";
    const requestedMode = urlState.get("mode") || (requestedDisplay ? "simulator" : "canvas");

    function toDisplayName(name) {
      if (!name) return "-";
      if (name.includes("_")) {
        return name
          .split("_")
          .filter(Boolean)
          .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
          .join(" ");
      }
      return name;
    }

    function pickDefaultEffect(runnable) {
      const preferred = ["color_wave", "rainbow", "gradient", "audio_pulse", "breathing", "solid_color"];
      for (const name of preferred) {
        const match = runnable.find((effect) => effect.name === name);
        if (match) return match.id;
      }
      return runnable[0]?.id || "";
    }

    function sortEffects(effects) {
      return [...effects].sort((left, right) => {
        const leftDisplay = toDisplayName(left?.name || "");
        const rightDisplay = toDisplayName(right?.name || "");
        const byDisplay = leftDisplay.localeCompare(rightDisplay, undefined, {
          sensitivity: "base",
          numeric: true,
        });
        if (byDisplay !== 0) return byDisplay;
        return (left?.name || "").localeCompare(right?.name || "", undefined, {
          sensitivity: "base",
          numeric: true,
        });
      });
    }

    function updateApplyButtonState() {
      const selected = selectEl.selectedOptions[0];
      const unavailable = selected?.disabled === true;
      applyBtn.disabled = !selected || !selectEl.value || unavailable;
    }

    function log(line) {
      const stamp = new Date().toISOString().split("T")[1].replace("Z", "");
      logEl.textContent = `[${stamp}] ${line}\n` + logEl.textContent;
      logEl.textContent = logEl.textContent.slice(0, 6000);
    }

    function setWsState(label, cls) {
      stateEl.textContent = label;
      stateEl.className = cls;
    }

    function resetFrameCount() {
      frameCount = 0;
      frameCountEl.textContent = "0";
    }

    function clearCanvas(width = 320, height = 200, message = "") {
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }
      ctx.fillStyle = "#000";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      if (message) {
        ctx.fillStyle = "#8fb0c4";
        ctx.font = '16px "JetBrains Mono", monospace';
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText(message, canvas.width / 2, canvas.height / 2);
      }
    }

    function selectedSimulator() {
      return simulatorConfigs.find((config) => config.id === simulatorEl.value) || null;
    }

    function closeWs() {
      if (!ws) return;
      const socket = ws;
      ws = null;
      socket.onopen = null;
      socket.onclose = null;
      socket.onerror = null;
      socket.onmessage = null;
      try { socket.close(); } catch (_) {}
    }

    function stopSimulatorPolling() {
      if (!simulatorPollHandle) return;
      clearInterval(simulatorPollHandle);
      simulatorPollHandle = null;
    }

    function syncPreviewQuery() {
      const next = new URLSearchParams(window.location.search);
      next.set("mode", modeEl.value);
      if (modeEl.value === "simulator" && simulatorEl.value) {
        next.set("display", simulatorEl.value);
      } else {
        next.delete("display");
      }
      const query = next.toString();
      const nextUrl = query ? `${window.location.pathname}?${query}` : window.location.pathname;
      window.history.replaceState(null, "", nextUrl);
    }

    function updatePreviewUi() {
      const simulatorMode = modeEl.value === "simulator";
      const simulator = selectedSimulator();
      modeLabelEl.textContent = simulatorMode ? "simulator" : "canvas";
      simulatorEl.disabled = !simulatorMode || simulatorConfigs.length === 0;
      fpsEl.disabled = simulatorMode;
      document.getElementById("reconnectBtn").textContent = simulatorMode ? "Refresh Frame" : "Reconnect";
      canvas.classList.toggle("circular-mask", simulatorMode && !!simulator?.circular);
      if (simulatorMode && simulator) {
        canvas.style.aspectRatio = `${simulator.width} / ${simulator.height}`;
        canvasSizeEl.textContent = `${simulator.width}x${simulator.height}`;
      } else {
        canvas.classList.remove("circular-mask");
        canvas.style.aspectRatio = "16 / 10";
      }
      syncPreviewQuery();
    }

    function apiHeaders() {
      return token ? { "Authorization": `Bearer ${token}` } : {};
    }

    async function loadEffects() {
      const previousSelection = selectEl.value;
      const res = await fetch("/api/v1/effects", { headers: apiHeaders() });
      if (!res.ok) throw new Error(`effects list failed (${res.status})`);
      const body = await res.json();
      const items = sortEffects(body?.data?.items || []);
      const runnable = items.filter((effect) => effect.runnable !== false);
      const blocked = items.filter((effect) => effect.runnable === false);
      const blockedHtml = blocked.filter((effect) => effect.source === "html");
      const visible = showUnavailableEl.checked ? items : runnable;

      selectEl.innerHTML = "";
      for (const effect of visible) {
        const unavailable = effect.runnable === false;
        const option = document.createElement("option");
        option.value = effect.id;
        option.dataset.name = effect.name;
        option.textContent = `${toDisplayName(effect.name)} (${effect.source || "unknown"})${unavailable ? " · unavailable" : ""}`;
        option.disabled = unavailable;
        selectEl.appendChild(option);
      }
      if (visible.length === 0) {
        const option = document.createElement("option");
        option.value = "";
        option.textContent = "no effects available";
        selectEl.appendChild(option);
      }
      if (selectEl.options.length > 0) {
        const previousIndex = Array.from(selectEl.options).findIndex((option) => option.value === previousSelection && !option.disabled);
        if (previousIndex >= 0) {
          selectEl.selectedIndex = previousIndex;
          updateApplyButtonState();
        }
      }
      if (!selectEl.value && runnable.length > 0) {
        const preferredId = pickDefaultEffect(runnable);
        const preferredIndex = Array.from(selectEl.options)
          .findIndex((option) => option.value === preferredId && !option.disabled);
        if (preferredIndex >= 0) {
          selectEl.selectedIndex = preferredIndex;
        } else {
          const firstRunnableIndex = Array.from(selectEl.options).findIndex((option) => !option.disabled);
          selectEl.selectedIndex = firstRunnableIndex >= 0 ? firstRunnableIndex : 0;
        }
      }
      updateApplyButtonState();

      if (blockedHtml.length > 0) {
        rendererHintEl.className = "hint status-warn";
        rendererHintEl.innerHTML = `HTML effects unavailable in this daemon build (${blockedHtml.length}). Restart with <code>${SERVO_RUN_HINT}</code> to enable Servo rendering.`;
      } else {
        rendererHintEl.className = "hint status-ok";
        rendererHintEl.textContent = "Servo HTML rendering is available in this daemon build.";
      }

      log(`loaded ${items.length} effect(s), runnable: ${runnable.length}`);
      if (blocked.length > 0) {
        const hiddenWord = showUnavailableEl.checked ? "shown as unavailable" : "hidden";
        log(`${blocked.length} effect(s) unavailable in this daemon build (${hiddenWord}; likely requires servo)`);
      }
    }

    async function loadSimulators() {
      const previousSelection = simulatorEl.value;
      const res = await fetch("/api/v1/simulators/displays", { headers: apiHeaders() });
      if (!res.ok) throw new Error(`simulator list failed (${res.status})`);
      const body = await res.json();
      simulatorConfigs = [...(body?.data || [])].sort((left, right) =>
        (left?.name || "").localeCompare(right?.name || "", undefined, {
          sensitivity: "base",
          numeric: true,
        })
      );

      simulatorEl.innerHTML = "";
      for (const simulator of simulatorConfigs) {
        const option = document.createElement("option");
        option.value = simulator.id;
        option.textContent = `${simulator.name} (${simulator.width}x${simulator.height}${simulator.circular ? ", circle" : ""})`;
        simulatorEl.appendChild(option);
      }
      if (simulatorConfigs.length === 0) {
        const option = document.createElement("option");
        option.value = "";
        option.textContent = "no simulators configured";
        simulatorEl.appendChild(option);
      }

      const preferredId = previousSelection || requestedDisplay;
      if (preferredId) {
        const preferredIndex = Array.from(simulatorEl.options).findIndex((option) => option.value === preferredId);
        if (preferredIndex >= 0) {
          simulatorEl.selectedIndex = preferredIndex;
        }
      }
      if (!simulatorEl.value && simulatorConfigs.length > 0) {
        simulatorEl.selectedIndex = 0;
      }
      updatePreviewUi();
      log(`loaded ${simulatorConfigs.length} simulator(s)`);
    }

    async function fetchActiveEffect() {
      const res = await fetch("/api/v1/effects/active", { headers: apiHeaders() });
      if (res.status === 404) {
        activeEffectEl.textContent = "-";
        return "";
      }
      if (!res.ok) throw new Error(`active effect failed (${res.status})`);
      const body = await res.json();
      const name = body?.data?.name || "-";
      activeEffectEl.textContent = toDisplayName(name);
      return name;
    }

    async function applySelectedEffect() {
      const effectId = selectEl.value;
      if (!effectId) return;
      const selectedOption = selectEl.selectedOptions[0];
      if (!selectedOption || selectedOption.disabled) {
        throw new Error("selected effect is unavailable in this build (requires servo)");
      }
      const selectedName = selectedOption.dataset?.name || effectId;

      const res = await fetch(`/api/v1/effects/${encodeURIComponent(effectId)}/apply`, {
        method: "POST",
        headers: { "Content-Type": "application/json", ...apiHeaders() },
        body: JSON.stringify({}),
      });
      if (!res.ok) {
        let details = "";
        try {
          const body = await res.json();
          details = body?.error?.message ? `: ${body.error.message}` : "";
        } catch (_) {}
        throw new Error(`apply failed (${res.status})${details}`);
      }
      await fetchActiveEffect();
      log(`applied effect '${toDisplayName(selectedName)}'`);
    }

    async function stopEffect() {
      const res = await fetch("/api/v1/effects/stop", {
        method: "POST",
        headers: apiHeaders(),
      });
      if (!res.ok && res.status !== 404) throw new Error(`stop failed (${res.status})`);
      await fetchActiveEffect();
      log("stopped active effect");
    }

    function connectWs() {
      if (modeEl.value !== "canvas") return;
      stopSimulatorPolling();
      if (ws) ws.close();

      const scheme = window.location.protocol === "https:" ? "wss" : "ws";
      const tokenQuery = token ? `?token=${encodeURIComponent(token)}` : "";
      const url = `${scheme}://${window.location.host}/api/v1/ws${tokenQuery}`;
      ws = new WebSocket(url, "hypercolor-v1");
      ws.binaryType = "arraybuffer";

      setWsState("connecting...", "status-warn");

      ws.onopen = () => {
        setWsState("connected", "status-ok");
        const fps = Math.max(1, Math.min(30, Number(fpsEl.value || 15)));
        ws.send(JSON.stringify({
          type: "subscribe",
          channels: ["canvas", "events"],
          config: { canvas: { fps, format: "rgba" } },
        }));
        log(`ws connected (${url})`);
      };

      ws.onclose = () => {
        setWsState("disconnected", "status-err");
        log("ws disconnected");
      };

      ws.onerror = () => {
        setWsState("error", "status-err");
        log("ws error");
      };

      ws.onmessage = (event) => {
        if (typeof event.data === "string") {
          handleJson(event.data);
          return;
        }
        if (event.data instanceof ArrayBuffer) {
          handleBinary(event.data);
        }
      };
    }

    async function refreshSimulatorFrame(options = {}) {
      if (modeEl.value !== "simulator") return;

      const quiet = options.quiet === true;
      const simulator = selectedSimulator();
      updatePreviewUi();
      if (!simulator) {
        clearCanvas(320, 200, "no simulator selected");
        setWsState("idle", "status-warn");
        return;
      }

      try {
        const res = await fetch(
          `/api/v1/simulators/displays/${encodeURIComponent(simulator.id)}/frame?ts=${Date.now()}`,
          { headers: apiHeaders() }
        );
        if (modeEl.value !== "simulator") return;
        if (res.status === 404) {
          clearCanvas(simulator.width, simulator.height, "waiting for first frame");
          setWsState("waiting", "status-warn");
          canvasSizeEl.textContent = `${simulator.width}x${simulator.height}`;
          return;
        }
        if (!res.ok) throw new Error(`simulator frame failed (${res.status})`);

        const blob = await res.blob();
        const bitmap = await createImageBitmap(blob);
        if (canvas.width !== simulator.width || canvas.height !== simulator.height) {
          canvas.width = simulator.width;
          canvas.height = simulator.height;
        }
        ctx.save();
        ctx.clearRect(0, 0, simulator.width, simulator.height);
        if (simulator.circular) {
          const radius = Math.min(simulator.width, simulator.height) / 2;
          ctx.beginPath();
          ctx.arc(simulator.width / 2, simulator.height / 2, radius, 0, Math.PI * 2);
          ctx.clip();
        }
        ctx.drawImage(bitmap, 0, 0, simulator.width, simulator.height);
        ctx.restore();
        if (typeof bitmap.close === "function") bitmap.close();

        frameCount += 1;
        frameCountEl.textContent = String(frameCount);
        canvasSizeEl.textContent = `${simulator.width}x${simulator.height}`;
        setWsState("simulator", "status-ok");
      } catch (err) {
        setWsState("error", "status-err");
        if (!quiet) log(`simulator frame error: ${String(err)}`);
      }
    }

    async function activatePreviewMode() {
      resetFrameCount();
      updatePreviewUi();

      if (modeEl.value === "simulator") {
        closeWs();
        stopSimulatorPolling();
        await refreshSimulatorFrame();
        simulatorPollHandle = setInterval(() => {
          refreshSimulatorFrame({ quiet: true });
        }, 1000);
        return;
      }

      stopSimulatorPolling();
      connectWs();
    }

    function handleJson(raw) {
      let msg;
      try { msg = JSON.parse(raw); } catch (_) { return; }
      if (msg.type === "hello") {
        if (msg.state?.effect?.name) activeEffectEl.textContent = toDisplayName(msg.state.effect.name);
        return;
      }
      if (msg.type === "event") {
        if (msg.event === "effect_started") {
          const started = msg.data?.effect?.name;
          activeEffectEl.textContent = started ? toDisplayName(started) : activeEffectEl.textContent;
        } else if (msg.event === "effect_stopped") {
          activeEffectEl.textContent = "-";
        }
      }
      if (msg.type === "error") {
        log(`ws error: ${msg.message || "unknown"}`);
      }
    }

    function handleBinary(buffer) {
      const bytes = new Uint8Array(buffer);
      if (bytes.length < 14) return;
      if (bytes[0] !== 0x03) return;

      const view = new DataView(buffer);
      const width = view.getUint16(9, true);
      const height = view.getUint16(11, true);
      const format = view.getUint8(13); // 0=rgb, 1=rgba
      const pixelData = bytes.subarray(14);
      const pixelCount = width * height;

      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }

      const imageData = ctx.createImageData(width, height);
      if (format === 1) {
        if (pixelData.length < pixelCount * 4) return;
        imageData.data.set(pixelData.subarray(0, pixelCount * 4));
      } else {
        if (pixelData.length < pixelCount * 3) return;
        for (let i = 0, j = 0; i < pixelCount; i++, j += 3) {
          const k = i * 4;
          imageData.data[k] = pixelData[j];
          imageData.data[k + 1] = pixelData[j + 1];
          imageData.data[k + 2] = pixelData[j + 2];
          imageData.data[k + 3] = 255;
        }
      }
      ctx.putImageData(imageData, 0, 0);

      frameCount += 1;
      frameCountEl.textContent = String(frameCount);
      canvasSizeEl.textContent = `${width}x${height}`;
    }

    document.getElementById("applyBtn").addEventListener("click", async () => {
      try { await applySelectedEffect(); } catch (err) { log(String(err)); }
    });
    document.getElementById("stopBtn").addEventListener("click", async () => {
      try { await stopEffect(); } catch (err) { log(String(err)); }
    });
    document.getElementById("reconnectBtn").addEventListener("click", async () => {
      try {
        if (modeEl.value === "simulator") {
          await loadSimulators();
          resetFrameCount();
          await refreshSimulatorFrame();
          return;
        }
        connectWs();
      } catch (err) { log(String(err)); }
    });
    fpsEl.addEventListener("change", () => {
      if (modeEl.value === "canvas") connectWs();
    });
    showUnavailableEl.addEventListener("change", async () => {
      try { await loadEffects(); } catch (err) { log(String(err)); }
    });
    selectEl.addEventListener("change", updateApplyButtonState);
    modeEl.addEventListener("change", async () => {
      try { await activatePreviewMode(); } catch (err) { log(String(err)); }
    });
    simulatorEl.addEventListener("change", async () => {
      try { await activatePreviewMode(); } catch (err) { log(String(err)); }
    });

    async function bootstrap() {
      try { await loadEffects(); } catch (err) { log(String(err)); }
      try { await loadSimulators(); } catch (err) { log(String(err)); }
      if (requestedMode === "simulator" && (requestedDisplay ? simulatorConfigs.some((config) => config.id === requestedDisplay) : simulatorConfigs.length > 0)) {
        modeEl.value = "simulator";
      }
      try {
        const active = await fetchActiveEffect();
        if (!active && selectEl.value) {
          await applySelectedEffect();
        }
      } catch (err) { log(String(err)); }
      try { await activatePreviewMode(); } catch (err) { log(String(err)); }
    }

    bootstrap();
  </script>
</body>
</html>
"##;
