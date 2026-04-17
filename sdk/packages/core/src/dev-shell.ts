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
        --radius: 22px;
        font-family: 'Inter', 'Segoe UI', sans-serif;
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
        grid-template-columns: minmax(0, 1.8fr) minmax(320px, 0.9fr);
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
        grid-template-rows: auto 1fr auto;
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
        padding: 22px;
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

      .sidebar {
        display: grid;
        grid-template-rows: auto auto auto 1fr;
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
        margin: 0 0 12px;
        font-size: 13px;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        color: var(--muted);
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
        transition: border-color 120ms ease, transform 120ms ease;
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
      .error {
        padding: 18px;
        border-radius: 16px;
        background: rgba(255, 255, 255, 0.03);
        color: var(--muted);
      }

      .error {
        color: var(--danger);
        border: 1px solid rgba(255, 99, 99, 0.2);
      }

      @media (max-width: 1080px) {
        .shell {
          grid-template-columns: 1fr;
        }

        .preview-stage {
          min-height: 320px;
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
            <div class="subtitle" id="subtitle">Loading workspace…</div>
          </div>
          <div id="status-pill" class="subtitle">Connecting</div>
        </div>
        <div class="preview-wrap">
          <div class="preview-stage" id="preview-stage">
            <iframe id="preview-frame" title="Effect preview"></iframe>
          </div>
        </div>
        <div class="footer">
          <div id="footer-entry">No effect selected</div>
          <div id="footer-size">640 × 480</div>
        </div>
      </section>

      <aside class="sidebar panel">
        <section class="card stack">
          <h2>Workspace</h2>
          <div class="row">
            <label>
              Effect
              <select id="effect-select"></select>
            </label>
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
      const appState = {
        canvasHeight: 480,
        canvasWidth: 640,
        controlValuesByEntry: {},
        entries: [],
        selectedId: null,
        socketState: 'connecting',
      }

      const refs = {
        canvasHeight: document.getElementById('canvas-height'),
        canvasWidth: document.getElementById('canvas-width'),
        controlsRoot: document.getElementById('controls-root'),
        effectSelect: document.getElementById('effect-select'),
        footerEntry: document.getElementById('footer-entry'),
        footerSize: document.getElementById('footer-size'),
        frame: document.getElementById('preview-frame'),
        presetGrid: document.getElementById('preset-grid'),
        previewStage: document.getElementById('preview-stage'),
        statusPill: document.getElementById('status-pill'),
        subtitle: document.getElementById('subtitle'),
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

      function applyControlsToFrame() {
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

        if (typeof frameWindow.update === 'function') {
          frameWindow.update(true)
        }
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
        refs.footerEntry.textContent = entry ? entry.name + ' · ' + entry.kind : 'No effect selected'
        refs.footerSize.textContent = appState.canvasWidth + ' × ' + appState.canvasHeight
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
            applyControlsToFrame()
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
            applyControlsToFrame()
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
            applyControlsToFrame()
          })
        } else if (control.type === 'color') {
          input = document.createElement('input')
          input.type = 'color'
          input.value = String(values[control.id] ?? '#80ffea')
          value.textContent = input.value
          input.addEventListener('input', () => {
            values[control.id] = input.value
            value.textContent = input.value
            applyControlsToFrame()
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
            applyControlsToFrame()
          })
        } else {
          input = document.createElement('input')
          input.type = 'text'
          input.value = control.type === 'rect' ? rectToString(values[control.id]) : String(values[control.id] ?? '')
          value.textContent = input.value
          input.addEventListener('change', () => {
            values[control.id] = control.type === 'rect' ? parseRect(input.value) : input.value
            value.textContent = input.value
            applyControlsToFrame()
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
          applyControlsToFrame()
        }
      }

      function render() {
        renderEffectSelect()
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

      refs.effectSelect.addEventListener('change', () => {
        setSelectedEntry(refs.effectSelect.value)
      })

      refs.canvasWidth.addEventListener('change', () => {
        appState.canvasWidth = Math.max(100, Number(refs.canvasWidth.value) || 640)
        updateFooter()
        reloadPreview()
      })

      refs.canvasHeight.addEventListener('change', () => {
        appState.canvasHeight = Math.max(100, Number(refs.canvasHeight.value) || 480)
        updateFooter()
        reloadPreview()
      })

      refs.frame.addEventListener('load', () => {
        applyControlsToFrame()
      })

      loadState().catch((error) => {
        refs.controlsRoot.innerHTML = '<div class="error">' + String(error) + '</div>'
      })
      connectSocket()
    </script>
  </body>
</html>`
}
