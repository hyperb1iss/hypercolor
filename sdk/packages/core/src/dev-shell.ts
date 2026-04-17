export function renderDevShell(): string {
    return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Hypercolor Effect Studio</title>
    <style>
      :root {
        color-scheme: dark;
        --bg: #090b12;
        --panel: rgba(12, 16, 28, 0.9);
        --panel-strong: rgba(16, 22, 38, 0.98);
        --line: rgba(128, 255, 234, 0.14);
        --accent: #80ffea;
        --accent-strong: #e135ff;
        --ink: #f4f7ff;
        --muted: #94a1c6;
        --danger: #ff6363;
        --success: #50fa7b;
        --radius: 22px;
        font-family: 'Avenir Next', 'Segoe UI', sans-serif;
      }

      * {
        box-sizing: border-box;
      }

      body {
        margin: 0;
        min-height: 100vh;
        background:
          radial-gradient(circle at top, rgba(225, 53, 255, 0.16), transparent 34%),
          radial-gradient(circle at bottom right, rgba(128, 255, 234, 0.12), transparent 30%),
          var(--bg);
        color: var(--ink);
      }

      .shell {
        display: grid;
        grid-template-columns: minmax(0, 1.8fr) minmax(340px, 0.92fr);
        gap: 20px;
        min-height: 100vh;
        padding: 22px;
      }

      .panel {
        border: 1px solid var(--line);
        border-radius: var(--radius);
        background: var(--panel);
        backdrop-filter: blur(24px);
        box-shadow: 0 22px 80px rgba(0, 0, 0, 0.45);
      }

      .preview-panel {
        display: grid;
        grid-template-rows: auto minmax(420px, 1fr) auto auto;
        overflow: hidden;
      }

      .header,
      .footer {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        padding: 18px 22px;
        border-bottom: 1px solid var(--line);
      }

      .footer {
        border-bottom: 0;
        border-top: 1px solid var(--line);
        color: var(--muted);
        font-size: 13px;
      }

      .header h1 {
        margin: 0;
        font-size: 18px;
        font-weight: 650;
        letter-spacing: 0.03em;
      }

      .subtitle {
        color: var(--muted);
        font-size: 13px;
      }

      .preview-wrap {
        display: grid;
        place-items: center;
        padding: 22px 22px 12px;
      }

      .preview-stage {
        display: grid;
        place-items: center;
        width: 100%;
        height: 100%;
        min-height: 420px;
        border-radius: 18px;
        border: 1px solid rgba(128, 255, 234, 0.1);
        background:
          linear-gradient(180deg, rgba(16, 22, 38, 0.96) 0%, rgba(9, 11, 18, 0.98) 100%);
      }

      iframe {
        border: 0;
        width: 100%;
        height: 100%;
        border-radius: 16px;
        background: #000;
      }

      .led-panel {
        display: grid;
        gap: 14px;
        padding: 0 22px 18px;
      }

      .led-panel-head {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
      }

      .led-kicker {
        color: var(--muted);
        font-size: 11px;
        font-weight: 700;
        letter-spacing: 0.12em;
        text-transform: uppercase;
      }

      .pill-row {
        display: flex;
        flex-wrap: wrap;
        justify-content: flex-end;
        gap: 8px;
      }

      .pill {
        border: 1px solid rgba(128, 255, 234, 0.14);
        border-radius: 999px;
        padding: 7px 12px;
        background: rgba(16, 22, 38, 0.88);
        color: var(--ink);
        font-size: 12px;
      }

      #led-preview-canvas {
        width: 100%;
        height: 190px;
        border-radius: 18px;
        border: 1px solid rgba(128, 255, 234, 0.1);
        background:
          radial-gradient(circle at top, rgba(225, 53, 255, 0.1), transparent 36%),
          linear-gradient(180deg, rgba(14, 18, 31, 0.98) 0%, rgba(8, 10, 18, 0.98) 100%);
      }

      .sidebar {
        display: grid;
        grid-template-rows: auto auto auto auto 1fr;
        gap: 16px;
        align-content: start;
        padding: 18px;
      }

      .card {
        border: 1px solid var(--line);
        border-radius: 18px;
        background: var(--panel-strong);
        padding: 16px;
      }

      .card h2 {
        margin: 0;
        font-size: 13px;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        color: var(--muted);
      }

      .card-head {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 12px;
      }

      .metric {
        min-width: 92px;
        border: 1px solid rgba(128, 255, 234, 0.12);
        border-radius: 14px;
        padding: 8px 10px;
        background: rgba(10, 12, 22, 0.72);
        color: var(--muted);
        font-size: 11px;
        text-transform: uppercase;
        letter-spacing: 0.08em;
      }

      .metric strong {
        display: block;
        margin-top: 4px;
        color: var(--ink);
        font-size: 15px;
        letter-spacing: normal;
      }

      .stack {
        display: grid;
        gap: 12px;
      }

      .row {
        display: grid;
        gap: 8px;
      }

      .row.two {
        grid-template-columns: repeat(2, minmax(0, 1fr));
      }

      label {
        display: grid;
        gap: 6px;
        font-size: 13px;
        color: var(--muted);
      }

      input,
      select,
      button,
      textarea {
        border: 1px solid rgba(128, 255, 234, 0.18);
        border-radius: 12px;
        background: rgba(9, 11, 18, 0.92);
        color: var(--ink);
        padding: 10px 12px;
        font: inherit;
      }

      input[type='checkbox'] {
        width: 18px;
        height: 18px;
        padding: 0;
      }

      input[type='range'] {
        padding: 0;
      }

      button {
        cursor: pointer;
        transition: border-color 120ms ease, transform 120ms ease, background 120ms ease;
      }

      button:hover {
        border-color: rgba(128, 255, 234, 0.4);
        transform: translateY(-1px);
      }

      .preset-grid {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
      }

      .preset-grid button {
        padding: 8px 12px;
      }

      .preset-grid.compact button {
        font-size: 12px;
      }

      .preset-grid button.active {
        border-color: rgba(225, 53, 255, 0.5);
        background: rgba(225, 53, 255, 0.14);
      }

      .control-inline {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        align-items: center;
        gap: 12px;
      }

      .group {
        display: grid;
        gap: 10px;
        padding: 14px;
        border-radius: 16px;
        background: rgba(255, 255, 255, 0.02);
        border: 1px solid rgba(128, 255, 234, 0.08);
      }

      .group-title {
        font-size: 12px;
        font-weight: 700;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        color: var(--muted);
      }

      .control-header {
        display: flex;
        justify-content: space-between;
        gap: 12px;
        font-size: 13px;
      }

      .control-value {
        color: var(--ink);
        font-variant-numeric: tabular-nums;
      }

      .boolean-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
      }

      .empty,
      .error,
      .input-note {
        padding: 14px 16px;
        border-radius: 16px;
        background: rgba(255, 255, 255, 0.03);
        color: var(--muted);
        font-size: 12px;
        line-height: 1.55;
      }

      .error {
        color: var(--danger);
        border: 1px solid rgba(255, 99, 99, 0.2);
      }

      .input-note {
        border: 1px solid rgba(128, 255, 234, 0.08);
      }

      @media (max-width: 1080px) {
        .shell {
          grid-template-columns: 1fr;
        }

        .preview-panel {
          grid-template-rows: auto minmax(320px, 1fr) auto auto;
        }

        .preview-stage {
          min-height: 320px;
        }

        .led-panel-head,
        .card-head {
          flex-direction: column;
          align-items: flex-start;
        }

        .pill-row {
          justify-content: flex-start;
        }
      }
    </style>
  </head>
  <body>
    <div class="shell">
      <section class="panel preview-panel">
        <div class="header">
          <div>
            <h1>Hypercolor Effect Studio</h1>
            <div class="subtitle" id="subtitle">Loading workspace...</div>
          </div>
          <div id="status-pill" class="subtitle">Connecting</div>
        </div>
        <div class="preview-wrap">
          <div class="preview-stage" id="preview-stage">
            <iframe id="preview-frame" title="Effect preview"></iframe>
          </div>
        </div>
        <section class="led-panel">
          <div class="led-panel-head">
            <div>
              <div class="led-kicker">LED Preview</div>
              <div class="subtitle">Sample the live canvas as a strip, matrix, or ring.</div>
            </div>
            <div class="pill-row">
              <div class="pill" id="led-layout-pill">Matrix</div>
              <div class="pill" id="led-count-pill">192 LEDs</div>
            </div>
          </div>
          <canvas id="led-preview-canvas" width="960" height="220"></canvas>
        </section>
        <div class="footer">
          <div id="footer-entry">No effect selected</div>
          <div id="footer-size">640 x 480</div>
        </div>
      </section>

      <aside class="sidebar panel">
        <section class="card stack">
          <div class="card-head">
            <h2>Workspace</h2>
            <div class="metric">
              Canvas
              <strong id="canvas-preset-label">Daemon</strong>
            </div>
          </div>
          <div class="row">
            <label>
              Effect
              <select id="effect-select"></select>
            </label>
          </div>
          <div class="row">
            <div class="preset-grid compact" id="canvas-preset-grid">
              <button type="button" data-canvas-preset="daemon">Daemon 640 x 480</button>
              <button type="button" data-canvas-preset="strip">Strip 1200 x 160</button>
              <button type="button" data-canvas-preset="matrix">Matrix 720 x 720</button>
              <button type="button" data-canvas-preset="ring">Ring 560 x 560</button>
            </div>
          </div>
          <div class="row two">
            <label>
              Width
              <input id="canvas-width" min="100" step="1" type="number" value="640" />
            </label>
            <label>
              Height
              <input id="canvas-height" min="100" step="1" type="number" value="480" />
            </label>
          </div>
        </section>

        <section class="card stack">
          <div class="card-head">
            <h2>Audio Simulation</h2>
            <div class="metric">
              Level
              <strong id="audio-level">0%</strong>
            </div>
          </div>
          <label>
            Bass
            <div class="control-inline">
              <input id="audio-bass" max="1" min="0" step="0.01" type="range" value="0.46" />
              <span class="control-value" id="audio-bass-value">0.46</span>
            </div>
          </label>
          <label>
            Mid
            <div class="control-inline">
              <input id="audio-mid" max="1" min="0" step="0.01" type="range" value="0.32" />
              <span class="control-value" id="audio-mid-value">0.32</span>
            </div>
          </label>
          <label>
            Treble
            <div class="control-inline">
              <input id="audio-treble" max="1" min="0" step="0.01" type="range" value="0.38" />
              <span class="control-value" id="audio-treble-value">0.38</span>
            </div>
          </label>
          <div class="row two">
            <label>
              Tempo
              <div class="control-inline">
                <input id="audio-tempo" max="180" min="60" step="1" type="range" value="124" />
                <span class="control-value" id="audio-tempo-value">124</span>
              </div>
            </label>
            <label>
              Stereo Width
              <div class="control-inline">
                <input id="audio-width" max="1" min="0" step="0.01" type="range" value="0.54" />
                <span class="control-value" id="audio-width-value">0.54</span>
              </div>
            </label>
          </div>
          <div class="row two">
            <label>
              Motion
              <div class="control-inline">
                <input id="audio-motion" max="1" min="0" step="0.01" type="range" value="0.34" />
                <span class="control-value" id="audio-motion-value">0.34</span>
              </div>
            </label>
            <button id="audio-beat" type="button">Trigger Beat</button>
          </div>
          <div class="input-note">
            Drive audio-reactive effects without live capture. Bass, mid, treble, tempo,
            and beat pulses feed the iframe's engine audio object in real time.
          </div>
        </section>

        <section class="card stack">
          <div class="card-head">
            <h2>LED Preview</h2>
            <div class="metric">
              Layout
              <strong id="led-config-label">Matrix</strong>
            </div>
          </div>
          <div class="row two">
            <label>
              Shape
              <select id="led-layout">
                <option value="strip">Strip</option>
                <option value="matrix" selected>Matrix</option>
                <option value="ring">Ring</option>
              </select>
            </label>
            <label>
              Strip LEDs
              <input id="led-strip-count" max="300" min="8" step="1" type="number" value="60" />
            </label>
          </div>
          <div class="row two">
            <label>
              Matrix Columns
              <input id="led-matrix-columns" max="64" min="2" step="1" type="number" value="16" />
            </label>
            <label>
              Matrix Rows
              <input id="led-matrix-rows" max="64" min="2" step="1" type="number" value="12" />
            </label>
          </div>
          <div class="row two">
            <label>
              Ring LEDs
              <input id="led-ring-count" max="96" min="8" step="1" type="number" value="24" />
            </label>
            <div class="input-note">
              The preview applies a gentle gamma lift so LEDs feel closer to hardware than
              a flat canvas screenshot.
            </div>
          </div>
        </section>

        <section class="card">
          <h2>Presets</h2>
          <div class="preset-grid" id="preset-grid"></div>
        </section>

        <section class="card">
          <h2>Controls</h2>
          <div class="stack" id="controls-root"></div>
        </section>
      </aside>
    </div>

    <script>
      const CANVAS_PRESETS = {
        daemon: {
          height: 480,
          label: 'Daemon',
          layout: 'matrix',
          matrixColumns: 16,
          matrixRows: 12,
          ringCount: 24,
          stripCount: 60,
          width: 640,
        },
        strip: {
          height: 160,
          label: 'Strip',
          layout: 'strip',
          matrixColumns: 16,
          matrixRows: 12,
          ringCount: 24,
          stripCount: 60,
          width: 1200,
        },
        matrix: {
          height: 720,
          label: 'Matrix',
          layout: 'matrix',
          matrixColumns: 16,
          matrixRows: 16,
          ringCount: 24,
          stripCount: 60,
          width: 720,
        },
        ring: {
          height: 560,
          label: 'Ring',
          layout: 'ring',
          matrixColumns: 12,
          matrixRows: 12,
          ringCount: 24,
          stripCount: 60,
          width: 560,
        },
      }

      const appState = {
        audio: {
          bass: 0.46,
          lastBeatAt: 0,
          mid: 0.32,
          motion: 0.34,
          tempo: 124,
          treble: 0.38,
          width: 0.54,
        },
        canvasHeight: 480,
        canvasPreset: 'daemon',
        canvasWidth: 640,
        controlValuesByEntry: {},
        entries: [],
        ledPreview: {
          layout: 'matrix',
          matrixColumns: 16,
          matrixRows: 12,
          ringCount: 24,
          stripCount: 60,
        },
        selectedId: null,
        socketState: 'connecting',
      }

      const refs = {
        audioBass: document.getElementById('audio-bass'),
        audioBassValue: document.getElementById('audio-bass-value'),
        audioBeat: document.getElementById('audio-beat'),
        audioLevel: document.getElementById('audio-level'),
        audioMid: document.getElementById('audio-mid'),
        audioMidValue: document.getElementById('audio-mid-value'),
        audioMotion: document.getElementById('audio-motion'),
        audioMotionValue: document.getElementById('audio-motion-value'),
        audioTempo: document.getElementById('audio-tempo'),
        audioTempoValue: document.getElementById('audio-tempo-value'),
        audioTreble: document.getElementById('audio-treble'),
        audioTrebleValue: document.getElementById('audio-treble-value'),
        audioWidth: document.getElementById('audio-width'),
        audioWidthValue: document.getElementById('audio-width-value'),
        canvasHeight: document.getElementById('canvas-height'),
        canvasPresetButtons: Array.from(document.querySelectorAll('[data-canvas-preset]')),
        canvasPresetLabel: document.getElementById('canvas-preset-label'),
        canvasWidth: document.getElementById('canvas-width'),
        controlsRoot: document.getElementById('controls-root'),
        effectSelect: document.getElementById('effect-select'),
        footerEntry: document.getElementById('footer-entry'),
        footerSize: document.getElementById('footer-size'),
        frame: document.getElementById('preview-frame'),
        ledConfigLabel: document.getElementById('led-config-label'),
        ledCountPill: document.getElementById('led-count-pill'),
        ledLayout: document.getElementById('led-layout'),
        ledLayoutPill: document.getElementById('led-layout-pill'),
        ledMatrixColumns: document.getElementById('led-matrix-columns'),
        ledMatrixRows: document.getElementById('led-matrix-rows'),
        ledPreviewCanvas: document.getElementById('led-preview-canvas'),
        ledRingCount: document.getElementById('led-ring-count'),
        ledStripCount: document.getElementById('led-strip-count'),
        presetGrid: document.getElementById('preset-grid'),
        statusPill: document.getElementById('status-pill'),
        subtitle: document.getElementById('subtitle'),
      }

      function clamp(value, min, max) {
        return Math.min(max, Math.max(min, value))
      }

      function cloneValue(value) {
        return value == null ? value : JSON.parse(JSON.stringify(value))
      }

      function entryById(id) {
        return appState.entries.find((entry) => entry.id === id) ?? null
      }

      function currentEntry() {
        return entryById(appState.selectedId)
      }

      function ledLayoutLabel(layout) {
        return layout.charAt(0).toUpperCase() + layout.slice(1)
      }

      function currentLedCount() {
        if (appState.ledPreview.layout === 'strip') return appState.ledPreview.stripCount
        if (appState.ledPreview.layout === 'ring') return appState.ledPreview.ringCount
        return appState.ledPreview.matrixColumns * appState.ledPreview.matrixRows
      }

      function fallbackValue(control) {
        if (control.default !== undefined) return cloneValue(control.default)
        if (control.type === 'boolean') return false
        if (control.type === 'number' || control.type === 'hue') return control.min ?? 0
        if (control.type === 'color') return '#80ffea'
        return ''
      }

      function ensureControlState(entry) {
        const metadata = entry?.metadata
        if (!entry || !metadata) return {}

        const current = appState.controlValuesByEntry[entry.id] ?? {}
        const next = {}
        for (const control of metadata.controls) {
          next[control.id] = Object.prototype.hasOwnProperty.call(current, control.id)
            ? current[control.id]
            : fallbackValue(control)
        }
        appState.controlValuesByEntry[entry.id] = next
        return next
      }

      function rectToString(value) {
        if (!value || typeof value !== 'object') return ''
        return [value.x ?? 0, value.y ?? 0, value.width ?? 0, value.height ?? 0].join(', ')
      }

      function parseRect(value) {
        if (typeof value === 'object' && value) return value
        const parts = String(value)
          .split(',')
          .map((part) => Number(part.trim()))
          .filter((part) => Number.isFinite(part))
        if (parts.length !== 4) return { x: 0, y: 0, width: 1, height: 1 }
        return { x: parts[0], y: parts[1], width: parts[2], height: parts[3] }
      }

      function normalizeRuntimeValue(control, value) {
        if (control.type === 'number' || control.type === 'hue') return Number(value)
        if (control.type === 'boolean') return Boolean(value)
        if (control.type === 'rect') return parseRect(value)
        return value
      }

      function previewUrl(entry) {
        const params = new URLSearchParams({
          height: String(appState.canvasHeight),
          revision: String(entry.revision),
          width: String(appState.canvasWidth),
        })
        return '/preview/' + encodeURIComponent(entry.id) + '?' + params.toString()
      }

      function reloadPreview() {
        const entry = currentEntry()
        if (!entry) {
          refs.frame.removeAttribute('src')
          return
        }

        refs.frame.src = previewUrl(entry)
      }

      function applyControlsToFrame(forceUpdate) {
        const frameWindow = refs.frame.contentWindow
        const entry = currentEntry()
        if (!frameWindow || !entry?.metadata) return

        const values = ensureControlState(entry)
        for (const control of entry.metadata.controls) {
          frameWindow[control.id] = normalizeRuntimeValue(control, values[control.id])
        }

        if (frameWindow.engine) {
          frameWindow.engine.width = appState.canvasWidth
          frameWindow.engine.height = appState.canvasHeight
        }

        if (forceUpdate && typeof frameWindow.update === 'function') {
          frameWindow.update(true)
        }
      }

      function setSelectedEntry(id) {
        if (!id || !entryById(id)) return
        appState.selectedId = id
        ensureControlState(currentEntry())
        render()
        reloadPreview()
      }

      function updateSubtitle() {
        const entry = currentEntry()
        if (!entry) {
          refs.subtitle.textContent = 'No effects discovered in this workspace'
          return
        }

        refs.subtitle.textContent = entry.metadata?.description || entry.name
      }

      function updateFooter() {
        const entry = currentEntry()
        const preset = CANVAS_PRESETS[appState.canvasPreset]
        const presetLabel = preset ? preset.label : 'Custom'
        refs.footerEntry.textContent = entry ? entry.name + ' · ' + entry.kind + ' · ' + presetLabel : 'No effect selected'
        refs.footerSize.textContent =
          appState.canvasWidth + ' x ' + appState.canvasHeight + ' · ' + currentLedCount() + ' LEDs'
      }

      function updateStatus() {
        refs.statusPill.textContent = appState.socketState
      }

      function renderEffectSelect() {
        refs.effectSelect.innerHTML = ''
        for (const entry of appState.entries) {
          const option = document.createElement('option')
          option.value = entry.id
          option.textContent = entry.name + (entry.kind === 'face' ? ' · face' : '')
          option.selected = entry.id === appState.selectedId
          refs.effectSelect.appendChild(option)
        }
      }

      function renderCanvasPresetButtons() {
        for (const button of refs.canvasPresetButtons) {
          const presetName = button.dataset.canvasPreset
          const active = presetName === appState.canvasPreset
          button.classList.toggle('active', active)
          button.setAttribute('aria-pressed', active ? 'true' : 'false')
        }
        refs.canvasPresetLabel.textContent =
          appState.canvasPreset === 'custom'
            ? 'Custom'
            : CANVAS_PRESETS[appState.canvasPreset]?.label ?? 'Custom'
      }

      function renderPresets() {
        refs.presetGrid.innerHTML = ''
        const entry = currentEntry()
        if (!entry?.metadata?.presets?.length) {
          const empty = document.createElement('div')
          empty.className = 'empty'
          empty.textContent = 'No presets declared'
          refs.presetGrid.appendChild(empty)
          return
        }

        for (const preset of entry.metadata.presets) {
          const button = document.createElement('button')
          button.type = 'button'
          button.textContent = preset.name
          button.addEventListener('click', () => {
            const values = ensureControlState(entry)
            appState.controlValuesByEntry[entry.id] = { ...values, ...preset.controls }
            renderControls()
            applyControlsToFrame(true)
          })
          refs.presetGrid.appendChild(button)
        }
      }

      function attachRangeValue(input, valueEl) {
        input.addEventListener('input', () => {
          valueEl.textContent = input.value
        })
      }

      function renderControl(control, values) {
        const row = document.createElement('div')
        row.className = 'row'

        if (control.type === 'boolean') {
          const wrapper = document.createElement('div')
          wrapper.className = 'boolean-row'

          const label = document.createElement('div')
          label.textContent = control.label || control.id
          wrapper.appendChild(label)

          const input = document.createElement('input')
          input.type = 'checkbox'
          input.checked = Boolean(values[control.id])
          input.addEventListener('change', () => {
            values[control.id] = input.checked
            applyControlsToFrame(true)
          })
          wrapper.appendChild(input)
          row.appendChild(wrapper)
          return row
        }

        const header = document.createElement('div')
        header.className = 'control-header'

        const label = document.createElement('span')
        label.textContent = control.label || control.id
        header.appendChild(label)

        const value = document.createElement('span')
        value.className = 'control-value'
        header.appendChild(value)
        row.appendChild(header)

        let input

        if (control.type === 'combobox') {
          input = document.createElement('select')
          for (const optionValue of control.values ?? []) {
            const option = document.createElement('option')
            option.value = optionValue
            option.textContent = optionValue
            input.appendChild(option)
          }
          input.value = String(values[control.id] ?? '')
          value.textContent = input.value
          input.addEventListener('change', () => {
            values[control.id] = input.value
            value.textContent = input.value
            applyControlsToFrame(true)
          })
        } else if (control.type === 'color') {
          input = document.createElement('input')
          input.type = 'color'
          input.value = String(values[control.id] ?? '#80ffea')
          value.textContent = input.value
          input.addEventListener('input', () => {
            values[control.id] = input.value
            value.textContent = input.value
            applyControlsToFrame(true)
          })
        } else if (control.type === 'number' || control.type === 'hue') {
          input = document.createElement('input')
          input.type = 'range'
          input.min = String(control.min ?? 0)
          input.max = String(control.max ?? 100)
          input.step = String(control.step ?? 1)
          input.value = String(values[control.id] ?? control.default ?? control.min ?? 0)
          value.textContent = input.value
          attachRangeValue(input, value)
          input.addEventListener('input', () => {
            values[control.id] = Number(input.value)
            applyControlsToFrame(true)
          })
        } else {
          input = document.createElement('input')
          input.type = 'text'
          input.value = control.type === 'rect' ? rectToString(values[control.id]) : String(values[control.id] ?? '')
          value.textContent = input.value
          input.addEventListener('change', () => {
            values[control.id] = control.type === 'rect' ? parseRect(input.value) : input.value
            value.textContent = input.value
            applyControlsToFrame(true)
          })
        }

        row.appendChild(input)
        return row
      }

      function renderControls() {
        refs.controlsRoot.innerHTML = ''
        const entry = currentEntry()
        if (!entry) {
          const empty = document.createElement('div')
          empty.className = 'empty'
          empty.textContent = 'No effect selected'
          refs.controlsRoot.appendChild(empty)
          return
        }

        if (entry.error) {
          const error = document.createElement('div')
          error.className = 'error'
          error.textContent = entry.error
          refs.controlsRoot.appendChild(error)
          return
        }

        if (!entry.metadata?.controls?.length) {
          const empty = document.createElement('div')
          empty.className = 'empty'
          empty.textContent = 'This effect has no controls'
          refs.controlsRoot.appendChild(empty)
          return
        }

        const values = ensureControlState(entry)
        const groups = new Map()
        for (const control of entry.metadata.controls) {
          const groupName = control.group || 'General'
          const controls = groups.get(groupName) || []
          controls.push(control)
          groups.set(groupName, controls)
        }

        for (const [groupName, controls] of groups) {
          const group = document.createElement('section')
          group.className = 'group'

          const title = document.createElement('div')
          title.className = 'group-title'
          title.textContent = groupName
          group.appendChild(title)

          for (const control of controls) {
            group.appendChild(renderControl(control, values))
          }

          refs.controlsRoot.appendChild(group)
        }
      }

      function renderLedConfig() {
        refs.ledLayout.value = appState.ledPreview.layout
        refs.ledStripCount.value = String(appState.ledPreview.stripCount)
        refs.ledMatrixColumns.value = String(appState.ledPreview.matrixColumns)
        refs.ledMatrixRows.value = String(appState.ledPreview.matrixRows)
        refs.ledRingCount.value = String(appState.ledPreview.ringCount)

        const layout = appState.ledPreview.layout
        refs.ledStripCount.disabled = layout !== 'strip'
        refs.ledMatrixColumns.disabled = layout !== 'matrix'
        refs.ledMatrixRows.disabled = layout !== 'matrix'
        refs.ledRingCount.disabled = layout !== 'ring'
        refs.ledConfigLabel.textContent = ledLayoutLabel(layout)
        refs.ledLayoutPill.textContent = ledLayoutLabel(layout)
        refs.ledCountPill.textContent = currentLedCount() + ' LEDs'
      }

      function renderAudioState() {
        refs.audioBass.value = String(appState.audio.bass)
        refs.audioBassValue.textContent = refs.audioBass.value
        refs.audioMid.value = String(appState.audio.mid)
        refs.audioMidValue.textContent = refs.audioMid.value
        refs.audioTreble.value = String(appState.audio.treble)
        refs.audioTrebleValue.textContent = refs.audioTreble.value
        refs.audioTempo.value = String(appState.audio.tempo)
        refs.audioTempoValue.textContent = refs.audioTempo.value
        refs.audioWidth.value = String(appState.audio.width)
        refs.audioWidthValue.textContent = refs.audioWidth.value
        refs.audioMotion.value = String(appState.audio.motion)
        refs.audioMotionValue.textContent = refs.audioMotion.value
      }

      function applyCanvasPreset(presetName, shouldReload = true) {
        const preset = CANVAS_PRESETS[presetName]
        if (!preset) return

        appState.canvasPreset = presetName
        appState.canvasWidth = preset.width
        appState.canvasHeight = preset.height
        appState.ledPreview.layout = preset.layout
        appState.ledPreview.stripCount = preset.stripCount
        appState.ledPreview.matrixColumns = preset.matrixColumns
        appState.ledPreview.matrixRows = preset.matrixRows
        appState.ledPreview.ringCount = preset.ringCount

        render()
        if (shouldReload) reloadPreview()
      }

      function mergeState(payload) {
        const previousEntry = currentEntry()
        const previousRevision = previousEntry?.revision

        appState.entries = payload.entries
        if (!appState.selectedId || !entryById(appState.selectedId)) {
          appState.selectedId = payload.initialSelectedId || payload.entries[0]?.id || null
        }

        ensureControlState(currentEntry())
        render()

        const nextEntry = currentEntry()
        if (!nextEntry) {
          refs.frame.removeAttribute('src')
          return
        }

        if (nextEntry.id !== previousEntry?.id || nextEntry.revision !== previousRevision) {
          reloadPreview()
        } else {
          applyControlsToFrame(true)
        }
      }

      function render() {
        renderEffectSelect()
        renderCanvasPresetButtons()
        renderAudioState()
        renderLedConfig()
        renderPresets()
        renderControls()
        updateSubtitle()
        updateFooter()
        updateStatus()
        refs.canvasWidth.value = String(appState.canvasWidth)
        refs.canvasHeight.value = String(appState.canvasHeight)
      }

      async function loadState() {
        const response = await fetch('/api/state')
        const payload = await response.json()
        mergeState(payload)
      }

      function connectSocket() {
        const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
        const socket = new WebSocket(protocol + '//' + location.host + '/ws')

        socket.addEventListener('open', () => {
          appState.socketState = 'Connected'
          updateStatus()
        })

        socket.addEventListener('close', () => {
          appState.socketState = 'Reconnecting'
          updateStatus()
          setTimeout(connectSocket, 500)
        })

        socket.addEventListener('message', (event) => {
          const payload = JSON.parse(event.data)
          if (payload.type === 'state') {
            mergeState(payload.state)
          }
        })
      }

      function computeAudioState(now) {
        const seconds = now / 1000
        const beatAge = appState.audio.lastBeatAt > 0 ? now - appState.audio.lastBeatAt : Number.POSITIVE_INFINITY
        const beatPulse = beatAge < 720 ? Math.exp(-beatAge / 210) : 0
        const motion = appState.audio.motion
        const motionWave = motion > 0
          ? ((Math.sin(seconds * 1.8) + Math.sin(seconds * 3.4 + 0.6)) * 0.5 * motion)
          : 0
        const bass = clamp(appState.audio.bass + motionWave * 0.16 + beatPulse * 0.24, 0, 1)
        const mid = clamp(appState.audio.mid + Math.sin(seconds * 2.1 + 1.8) * motion * 0.08, 0, 1)
        const treble = clamp(appState.audio.treble + Math.cos(seconds * 2.7 + 0.4) * motion * 0.1, 0, 1)
        const level = clamp(bass * 0.42 + mid * 0.34 + treble * 0.24 + beatPulse * 0.08, 0, 1)
        const width = clamp(appState.audio.width + Math.sin(seconds * 0.35) * 0.04, 0, 1)
        const harmonicHue = (((bass * 48) + (mid * 158) + (treble * 282) + seconds * 18) % 360 + 360) % 360
        const tempo = appState.audio.tempo

        return {
          bass,
          bassEnv: clamp(bass * 0.92 + beatPulse * 0.1, 0, 1),
          beat: beatPulse > 0.7 ? 1 : 0,
          beatConfidence: beatPulse > 0 ? 0.9 : 0.36,
          beatPhase: ((seconds * tempo) / 60) % 1,
          beatPulse,
          brightness: clamp(0.22 + treble * 0.6, 0, 1),
          chordMood: clamp(mid - bass * 0.48, -1, 1),
          density: clamp(level * 0.88 + motion * 0.12, 0, 1),
          dominantPitch: Math.round(harmonicHue / 30) % 12,
          dominantPitchConfidence: clamp(0.28 + level * 0.5, 0, 1),
          harmonicHue,
          level,
          levelLong: clamp(level * 0.82, 0, 1),
          levelShort: clamp(level * 1.05, 0, 1),
          momentum: clamp((mid - bass) * 0.55, -1, 1),
          onset: beatPulse > 0.82 ? 1 : 0,
          onsetPulse: clamp(beatPulse * 0.94, 0, 1),
          rolloff: clamp(0.46 + treble * 0.32, 0, 1),
          roughness: clamp(0.15 + Math.abs(mid - treble) * 0.45, 0, 1),
          spectralFlux: clamp(Math.abs(motionWave) * 0.85 + beatPulse * 0.22, 0, 1),
          spread: clamp(0.18 + width * 0.58, 0, 1),
          swell: clamp(beatPulse + motion * 0.18, 0, 1),
          tempo,
          treble,
          trebleEnv: clamp(treble * 0.94 + motion * 0.04, 0, 1),
          width,
          mid,
          midEnv: clamp(mid * 0.93 + motion * 0.05, 0, 1),
        }
      }

      function ensureAudioArray(target, key, Ctor, length) {
        const candidate = target[key]
        if (!candidate || typeof candidate.length !== 'number' || candidate.length !== length || typeof candidate.fill !== 'function') {
          target[key] = new Ctor(length)
        }
        return target[key]
      }

      function syncAudioToFrame(now) {
        const state = computeAudioState(now)
        refs.audioLevel.textContent = Math.round(state.level * 100) + '%'

        const frameWindow = refs.frame.contentWindow
        if (!frameWindow?.engine?.audio) {
          return state
        }

        if (frameWindow.engine) {
          frameWindow.engine.width = appState.canvasWidth
          frameWindow.engine.height = appState.canvasHeight
        }

        const audio = frameWindow.engine.audio
        audio.bass = state.bass
        audio.bassEnv = state.bassEnv
        audio.beat = state.beat
        audio.beatConfidence = state.beatConfidence
        audio.beatPhase = state.beatPhase
        audio.beatPulse = state.beatPulse
        audio.brightness = state.brightness
        audio.chordMood = state.chordMood
        audio.density = state.density
        audio.dominantPitch = state.dominantPitch
        audio.dominantPitchConfidence = state.dominantPitchConfidence
        audio.harmonicHue = state.harmonicHue
        audio.level = state.level
        audio.levelLong = state.levelLong
        audio.levelRaw = Math.round(-100 + state.level * 100)
        audio.levelShort = state.levelShort
        audio.mid = state.mid
        audio.midEnv = state.midEnv
        audio.momentum = state.momentum
        audio.onset = state.onset
        audio.onsetPulse = state.onsetPulse
        audio.rolloff = state.rolloff
        audio.roughness = state.roughness
        audio.spectralFlux = state.spectralFlux
        audio.spread = state.spread
        audio.swell = state.swell
        audio.tempo = state.tempo
        audio.treble = state.treble
        audio.trebleEnv = state.trebleEnv
        audio.width = state.width

        const frequency = ensureAudioArray(audio, 'frequency', Float32Array, 200)
        const frequencyRaw = ensureAudioArray(audio, 'frequencyRaw', Int8Array, 200)
        const frequencyWeighted = ensureAudioArray(audio, 'frequencyWeighted', Float32Array, 200)
        for (let index = 0; index < frequency.length; index += 1) {
          const t = index / (frequency.length - 1)
          const bassWeight = clamp(1 - t / 0.34, 0, 1)
          const midWeight = clamp(1 - Math.abs(t - 0.5) / 0.26, 0, 1)
          const trebleWeight = clamp(1 - (1 - t) / 0.34, 0, 1)
          const ripple = 0.5 + 0.5 * Math.sin((now / 1000) * 4 + t * 18)
          const value = clamp(
            state.bass * bassWeight +
              state.mid * midWeight +
              state.treble * trebleWeight +
              ripple * appState.audio.motion * 0.14 +
              state.beatPulse * clamp(1 - Math.abs(t - 0.14) / 0.2, 0, 1) * 0.22,
            0,
            1
          )
          frequency[index] = value
          frequencyWeighted[index] = clamp(value * (0.82 + t * 0.2), 0, 1)
          frequencyRaw[index] = Math.round(value * 127)
        }

        const melBands = ensureAudioArray(audio, 'melBands', Float32Array, 24)
        const melBandsNormalized = ensureAudioArray(audio, 'melBandsNormalized', Float32Array, 24)
        for (let index = 0; index < melBands.length; index += 1) {
          const t = index / (melBands.length - 1)
          const value = clamp(
            state.bass * clamp(1 - t / 0.42, 0, 1) +
              state.mid * clamp(1 - Math.abs(t - 0.52) / 0.3, 0, 1) +
              state.treble * clamp(1 - (1 - t) / 0.3, 0, 1),
            0,
            1
          )
          melBands[index] = value
          melBandsNormalized[index] = clamp(value * (0.9 + state.beatPulse * 0.2), 0, 1)
        }

        const chromagram = ensureAudioArray(audio, 'chromagram', Float32Array, 12)
        for (let index = 0; index < chromagram.length; index += 1) {
          const hueAngle = (state.harmonicHue / 360) * Math.PI * 2
          chromagram[index] = clamp(
            0.2 +
              state.level * 0.4 +
              Math.sin(hueAngle + (index / chromagram.length) * Math.PI * 2) * 0.3,
            0,
            1
          )
        }

        const spectralFluxBands = ensureAudioArray(audio, 'spectralFluxBands', Float32Array, 3)
        spectralFluxBands[0] = clamp(state.bass * 0.62 + state.beatPulse * 0.2, 0, 1)
        spectralFluxBands[1] = clamp(state.mid * 0.58 + appState.audio.motion * 0.15, 0, 1)
        spectralFluxBands[2] = clamp(state.treble * 0.6 + appState.audio.motion * 0.12, 0, 1)

        return state
      }

      function drawLedPlaceholder(ctx, width, height, message) {
        ctx.clearRect(0, 0, width, height)
        ctx.fillStyle = 'rgba(9, 11, 18, 0.96)'
        ctx.fillRect(0, 0, width, height)
        ctx.fillStyle = '#94a1c6'
        ctx.font = '14px Avenir Next, sans-serif'
        ctx.textAlign = 'center'
        ctx.fillText(message, width / 2, height / 2)
      }

      function previewCanvasFromFrame() {
        const frameDocument = refs.frame.contentDocument
        if (!frameDocument) return null

        const exact = frameDocument.getElementById('exCanvas')
        if (exact && typeof exact.getContext === 'function') return exact

        const firstCanvas = frameDocument.querySelector('canvas')
        if (firstCanvas && typeof firstCanvas.getContext === 'function') return firstCanvas

        return null
      }

      function ledColorToCss(color, alpha = 1) {
        return 'rgba(' + color.r + ', ' + color.g + ', ' + color.b + ', ' + alpha + ')'
      }

      function gammaLift(channel) {
        const normalized = channel / 255
        return Math.round(Math.pow(normalized, 0.78) * 255)
      }

      function sampleColor(ctx, width, height, xNorm, yNorm) {
        const x = clamp(Math.round(xNorm * (width - 1)), 0, width - 1)
        const y = clamp(Math.round(yNorm * (height - 1)), 0, height - 1)
        const pixel = ctx.getImageData(x, y, 1, 1).data
        return {
          a: (pixel[3] ?? 255) / 255,
          b: gammaLift(pixel[2] ?? 0),
          g: gammaLift(pixel[1] ?? 0),
          r: gammaLift(pixel[0] ?? 0),
        }
      }

      function drawLed(ctx, x, y, radius, color) {
        ctx.beginPath()
        ctx.fillStyle = ledColorToCss(color, 0.22)
        ctx.arc(x, y, radius * 1.7, 0, Math.PI * 2)
        ctx.fill()

        ctx.beginPath()
        ctx.shadowBlur = radius * 1.8
        ctx.shadowColor = ledColorToCss(color, 0.95)
        ctx.fillStyle = ledColorToCss(color, 1)
        ctx.arc(x, y, radius, 0, Math.PI * 2)
        ctx.fill()
        ctx.shadowBlur = 0
      }

      function renderLedPreview() {
        const canvas = refs.ledPreviewCanvas
        const ctx = canvas.getContext('2d')
        if (!ctx) return

        const width = canvas.width
        const height = canvas.height
        ctx.clearRect(0, 0, width, height)
        ctx.fillStyle = 'rgba(9, 11, 18, 0.98)'
        ctx.fillRect(0, 0, width, height)

        const sourceCanvas = previewCanvasFromFrame()
        if (!sourceCanvas) {
          drawLedPlaceholder(ctx, width, height, 'Canvas preview unavailable for this effect')
          return
        }

        const sourceCtx = sourceCanvas.getContext('2d', { willReadFrequently: true })
        if (!sourceCtx) {
          drawLedPlaceholder(ctx, width, height, 'Unable to sample the preview canvas')
          return
        }

        const layout = appState.ledPreview.layout
        refs.ledLayoutPill.textContent = ledLayoutLabel(layout)
        refs.ledCountPill.textContent = currentLedCount() + ' LEDs'

        if (layout === 'strip') {
          const count = appState.ledPreview.stripCount
          const usableWidth = width - 72
          const radius = Math.min(usableWidth / Math.max(count, 1) / 2.8, 15)
          const y = height / 2

          for (let index = 0; index < count; index += 1) {
            const xNorm = count === 1 ? 0.5 : index / (count - 1)
            const color = sampleColor(sourceCtx, sourceCanvas.width, sourceCanvas.height, xNorm, 0.5)
            const x = 36 + xNorm * usableWidth
            drawLed(ctx, x, y, radius, color)
          }

          return
        }

        if (layout === 'ring') {
          const count = appState.ledPreview.ringCount
          const radius = Math.min(width, height) * 0.33
          const ledRadius = Math.max(8, Math.min(16, (Math.PI * radius) / Math.max(count, 1) / 2.3))
          const centerX = width / 2
          const centerY = height / 2

          for (let index = 0; index < count; index += 1) {
            const theta = (-Math.PI / 2) + (index / count) * Math.PI * 2
            const x = centerX + Math.cos(theta) * radius
            const y = centerY + Math.sin(theta) * radius
            const sampleX = 0.5 + Math.cos(theta) * 0.34
            const sampleY = 0.5 + Math.sin(theta) * 0.34
            const color = sampleColor(sourceCtx, sourceCanvas.width, sourceCanvas.height, sampleX, sampleY)
            drawLed(ctx, x, y, ledRadius, color)
          }

          return
        }

        const columns = appState.ledPreview.matrixColumns
        const rows = appState.ledPreview.matrixRows
        const gap = 8
        const ledRadius = Math.min((width - 72) / Math.max(columns, 1) / 2.5, (height - 48) / Math.max(rows, 1) / 2.5)
        const gridWidth = columns * ledRadius * 2 + Math.max(columns - 1, 0) * gap
        const gridHeight = rows * ledRadius * 2 + Math.max(rows - 1, 0) * gap
        const startX = (width - gridWidth) / 2 + ledRadius
        const startY = (height - gridHeight) / 2 + ledRadius

        for (let row = 0; row < rows; row += 1) {
          for (let column = 0; column < columns; column += 1) {
            const sampleX = columns === 1 ? 0.5 : column / (columns - 1)
            const sampleY = rows === 1 ? 0.5 : row / (rows - 1)
            const color = sampleColor(sourceCtx, sourceCanvas.width, sourceCanvas.height, sampleX, sampleY)
            const x = startX + column * (ledRadius * 2 + gap)
            const y = startY + row * (ledRadius * 2 + gap)
            drawLed(ctx, x, y, ledRadius, color)
          }
        }
      }

      function animationLoop(now) {
        syncAudioToFrame(now)
        renderLedPreview()
        window.requestAnimationFrame(animationLoop)
      }

      refs.effectSelect.addEventListener('change', () => {
        setSelectedEntry(refs.effectSelect.value)
      })

      refs.canvasWidth.addEventListener('change', () => {
        appState.canvasPreset = 'custom'
        appState.canvasWidth = Math.max(100, Number(refs.canvasWidth.value) || 640)
        renderCanvasPresetButtons()
        updateFooter()
        reloadPreview()
      })

      refs.canvasHeight.addEventListener('change', () => {
        appState.canvasPreset = 'custom'
        appState.canvasHeight = Math.max(100, Number(refs.canvasHeight.value) || 480)
        renderCanvasPresetButtons()
        updateFooter()
        reloadPreview()
      })

      for (const button of refs.canvasPresetButtons) {
        button.addEventListener('click', () => {
          applyCanvasPreset(button.dataset.canvasPreset)
        })
      }

      refs.audioBass.addEventListener('input', () => {
        appState.audio.bass = clamp(Number(refs.audioBass.value) || 0, 0, 1)
        refs.audioBassValue.textContent = refs.audioBass.value
      })
      refs.audioMid.addEventListener('input', () => {
        appState.audio.mid = clamp(Number(refs.audioMid.value) || 0, 0, 1)
        refs.audioMidValue.textContent = refs.audioMid.value
      })
      refs.audioTreble.addEventListener('input', () => {
        appState.audio.treble = clamp(Number(refs.audioTreble.value) || 0, 0, 1)
        refs.audioTrebleValue.textContent = refs.audioTreble.value
      })
      refs.audioTempo.addEventListener('input', () => {
        appState.audio.tempo = clamp(Number(refs.audioTempo.value) || 120, 60, 180)
        refs.audioTempoValue.textContent = refs.audioTempo.value
      })
      refs.audioWidth.addEventListener('input', () => {
        appState.audio.width = clamp(Number(refs.audioWidth.value) || 0, 0, 1)
        refs.audioWidthValue.textContent = refs.audioWidth.value
      })
      refs.audioMotion.addEventListener('input', () => {
        appState.audio.motion = clamp(Number(refs.audioMotion.value) || 0, 0, 1)
        refs.audioMotionValue.textContent = refs.audioMotion.value
      })
      refs.audioBeat.addEventListener('click', () => {
        appState.audio.lastBeatAt = performance.now()
      })

      refs.ledLayout.addEventListener('change', () => {
        appState.ledPreview.layout = refs.ledLayout.value
        renderLedConfig()
        updateFooter()
      })
      refs.ledStripCount.addEventListener('change', () => {
        appState.ledPreview.stripCount = clamp(Number(refs.ledStripCount.value) || 60, 8, 300)
        renderLedConfig()
        updateFooter()
      })
      refs.ledMatrixColumns.addEventListener('change', () => {
        appState.ledPreview.matrixColumns = clamp(Number(refs.ledMatrixColumns.value) || 16, 2, 64)
        renderLedConfig()
        updateFooter()
      })
      refs.ledMatrixRows.addEventListener('change', () => {
        appState.ledPreview.matrixRows = clamp(Number(refs.ledMatrixRows.value) || 12, 2, 64)
        renderLedConfig()
        updateFooter()
      })
      refs.ledRingCount.addEventListener('change', () => {
        appState.ledPreview.ringCount = clamp(Number(refs.ledRingCount.value) || 24, 8, 96)
        renderLedConfig()
        updateFooter()
      })

      refs.frame.addEventListener('load', () => {
        applyControlsToFrame(true)
      })

      loadState().catch((error) => {
        refs.controlsRoot.innerHTML = '<div class="error">' + String(error) + '</div>'
      })
      connectSocket()
      window.requestAnimationFrame(animationLoop)
    </script>
  </body>
</html>`
}
