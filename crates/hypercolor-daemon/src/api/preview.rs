//! Live preview page for browser-based effect visualization.

use axum::response::{Html, IntoResponse, Response};

/// `GET /preview` — serve a lightweight browser UI for live canvas preview.
pub async fn preview_page() -> Response {
    Html(PREVIEW_HTML).into_response()
}

const PREVIEW_HTML: &str = r#"<!doctype html>
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

    .meta {
      display: flex;
      flex-wrap: wrap;
      gap: 14px;
      font-size: 12px;
      color: var(--muted);
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
        <select id="effectSelect"></select>
        <button id="applyBtn" type="button">Apply Effect</button>
        <button id="stopBtn" type="button">Stop</button>
        <input id="fpsInput" type="number" min="1" max="30" value="15" title="Canvas FPS" />
        <button id="reconnectBtn" type="button">Reconnect</button>
      </div>
      <div class="meta">
        <span>WS: <strong id="wsState" class="status-warn">connecting...</strong></span>
        <span>Frames: <strong id="frameCount">0</strong></span>
        <span>Size: <strong id="canvasSize">-</strong></span>
        <span>Effect: <strong id="activeEffect">-</strong></span>
      </div>
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
    const selectEl = document.getElementById("effectSelect");
    const fpsEl = document.getElementById("fpsInput");
    const logEl = document.getElementById("log");
    const canvas = document.getElementById("previewCanvas");
    const ctx = canvas.getContext("2d");

    let ws = null;
    let frameCount = 0;
    let token = new URLSearchParams(window.location.search).get("token") || "";

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

    function log(line) {
      const stamp = new Date().toISOString().split("T")[1].replace("Z", "");
      logEl.textContent = `[${stamp}] ${line}\n` + logEl.textContent;
      logEl.textContent = logEl.textContent.slice(0, 6000);
    }

    function setWsState(label, cls) {
      stateEl.textContent = label;
      stateEl.className = cls;
    }

    function apiHeaders() {
      return token ? { "Authorization": `Bearer ${token}` } : {};
    }

    async function loadEffects() {
      const res = await fetch("/api/v1/effects", { headers: apiHeaders() });
      if (!res.ok) throw new Error(`effects list failed (${res.status})`);
      const body = await res.json();
      const items = body?.data?.items || [];
      const runnable = items.filter((effect) => effect.runnable !== false);
      const blocked = items.filter((effect) => effect.runnable === false);

      selectEl.innerHTML = "";
      for (const effect of runnable) {
        const option = document.createElement("option");
        option.value = effect.id;
        option.dataset.name = effect.name;
        option.textContent = `${toDisplayName(effect.name)} (${effect.source || "unknown"})`;
        selectEl.appendChild(option);
      }
      if (runnable.length === 0) {
        const option = document.createElement("option");
        option.value = "";
        option.textContent = "no runnable effects available";
        selectEl.appendChild(option);
      }
      if (runnable.length > 0) {
        const preferredId = pickDefaultEffect(runnable);
        const preferredIndex = runnable.findIndex((effect) => effect.id === preferredId);
        selectEl.selectedIndex = preferredIndex >= 0 ? preferredIndex : 0;
      }

      log(`loaded ${items.length} effect(s), runnable: ${runnable.length}`);
      if (blocked.length > 0) {
        log(`${blocked.length} effect(s) unavailable in this daemon build (likely requires servo)`);
      }
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
      const selectedName = selectEl.selectedOptions[0]?.dataset?.name || effectId;

      const res = await fetch(`/api/v1/effects/${encodeURIComponent(effectId)}/apply`, {
        method: "POST",
        headers: { "Content-Type": "application/json", ...apiHeaders() },
        body: JSON.stringify({ transition: { type: "crossfade", duration_ms: 250 } }),
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
    document.getElementById("reconnectBtn").addEventListener("click", connectWs);
    fpsEl.addEventListener("change", connectWs);

    async function bootstrap() {
      try { await loadEffects(); } catch (err) { log(String(err)); }
      try {
        const active = await fetchActiveEffect();
        if (!active && selectEl.value) {
          await applySelectedEffect();
        }
      } catch (err) { log(String(err)); }
      connectWs();
    }

    bootstrap();
  </script>
</body>
</html>
"#;
