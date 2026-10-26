import { readFile } from 'node:fs/promises'

// Compare a reference shader with the working shader in a real WebGL2 browser.
// Usage: bun sdk/scripts/check-bubble-garden-gpu.mjs reference.glsl [candidate.glsl]
// Open the printed URL, then click Run GPU check or invoke window.runCheck().
// Starting after navigation keeps browser launch setup outside the measured run.
// The process exits after the browser posts its result.
const [referencePath, candidatePath] = process.argv.slice(2)
if (!referencePath) throw Error('Supply a reference Bubble Garden fragment shader')
const reference = await readFile(referencePath, 'utf8')
const candidate = await readFile(
    candidatePath ?? new URL('../src/effects/bubble-garden/fragment.glsl', import.meta.url),
    'utf8',
)
async function harness() {
    const state = { phase: 'initializing' }
    window.gpuCheckState = state
    const canvas = document.createElement('canvas')
    canvas.width = 640
    canvas.height = 630
    document.body.append(canvas)
    const antialias = new URLSearchParams(location.search).get('antialias') === 'true'
    const gl = canvas.getContext('webgl2', { antialias, preserveDrawingBuffer: true })
    if (!gl) throw Error('WebGL2 unavailable')
    canvas.addEventListener('webglcontextlost', (event) => {
        state.contextLoss = event.statusMessage || 'WebGL context lost'
    })
    const timer = gl.getExtension('EXT_disjoint_timer_query_webgl2')
    const debug = gl.getExtension('WEBGL_debug_renderer_info')
    const renderer = debug ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER)
    state.renderer = renderer
    state.contextAttributes = gl.getContextAttributes()
    const vertex =
        '#version 300 es\nvoid main(){vec2 p=vec2(float((gl_VertexID<<1)&2),float(gl_VertexID&2));gl_Position=vec4(p*2.-1.,0.,1.);}'
    function compile(type, source) {
        const shader = gl.createShader(type)
        gl.shaderSource(shader, source)
        gl.compileShader(shader)
        if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) throw Error(gl.getShaderInfoLog(shader))
        return shader
    }
    function program(source) {
        const p = gl.createProgram()
        gl.attachShader(p, compile(gl.VERTEX_SHADER, vertex))
        gl.attachShader(p, compile(gl.FRAGMENT_SHADER, source))
        gl.linkProgram(p)
        if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw Error(gl.getProgramInfoLog(p))
        return p
    }
    const baseline = program(window.shaders.reference)
    const optimized = program(window.shaders.candidate)
    const defaults = {
        background: [0.09, 0, 0.52],
        colors: [
            [0.03, 0.97, 1],
            [1, 0.02, 0.71],
            [0.44, 0.18, 1],
        ],
        count: 31.84,
        height: 630,
        mode: 1,
        size: 4.97,
        speed: 6,
        theme: 3,
        time: 31.25,
        width: 640,
    }
    function draw(p, input) {
        const c = { ...defaults, ...input }
        if (canvas.width !== c.width || canvas.height !== c.height) {
            canvas.width = c.width
            canvas.height = c.height
        }
        gl.viewport(0, 0, c.width, c.height)
        gl.useProgram(p)
        const loc = (n) => gl.getUniformLocation(p, n)
        gl.uniform2f(loc('iResolution'), c.width, c.height)
        for (const [name, value] of Object.entries({ iCount: c.count, iSize: c.size, iSpeed: c.speed, iTime: c.time }))
            gl.uniform1f(loc(name), value)
        gl.uniform1i(loc('iColorMode'), c.mode)
        gl.uniform1i(loc('iTheme'), c.theme)
        gl.uniform3fv(loc('iBgColor'), c.background)
        ;['iColor', 'iColor2', 'iColor3'].forEach((n, i) => {
            gl.uniform3fv(loc(n), c.colors[i])
        })
        gl.drawArrays(gl.TRIANGLES, 0, 3)
    }
    const next = () => new Promise((resolve) => setTimeout(resolve, 0))
    async function parity() {
        const cases = []
        for (let theme = 0; theme < 8; theme++)
            for (let mode = 0; mode < 4; mode++) {
                cases.push({
                    count: [10, 31.84, 96, 95.5][mode],
                    height: 127,
                    mode,
                    size: [1, 4.97, 10, 3.5][mode],
                    speed: [0, 6, 58, 100][mode],
                    theme,
                    time: theme * 3.7 + mode * 11.1,
                    width: 129,
                })
            }
        for (const [width, height] of [
            [640, 630],
            [640, 480],
            [960, 160],
            [127, 511],
            [1, 1],
            [2, 2],
            [3, 5],
            [31, 1],
            [1, 31],
        ]) {
            for (const count of [10, 31.49, 31.5, 31.84, 96])
                cases.push({ count, height, size: count === 96 ? 10 : 4.97, time: count === 10 ? 0 : 12345.67, width })
        }
        for (const time of [0, 0.0166667, 1, 5, 31.25, 50, 123.456, 3600, 86400])
            cases.push({ count: 96, size: 1, speed: 100, time })
        for (const colors of [
            [
                [0, 0, 0],
                [0, 0, 0],
                [0, 0, 0],
            ],
            [
                [1, 1, 1],
                [1, 1, 1],
                [1, 1, 1],
            ],
            [
                [1, 0, 0],
                [0, 1, 0],
                [0, 0, 1],
            ],
            [
                [0.002, 0.93, 0.99],
                [1, 0.002, 0.46],
                [0.159, 0.026, 1],
            ],
        ]) {
            for (const background of [
                [0, 0, 0],
                [1, 1, 1],
            ])
                cases.push({ background, colors, count: 96, height: 129, mode: 1, size: 10, theme: 2, width: 127 })
        }
        let maxChannelError = 0
        let unequalChannels = 0
        let totalChannels = 0
        const failures = []
        for (let index = 0; index < cases.length; index++) {
            const c = { ...defaults, ...cases[index] }
            state.phase = 'parity'
            state.caseIndex = index
            state.input = c
            const a = new Uint8Array(c.width * c.height * 4)
            const b = new Uint8Array(a.length)
            draw(baseline, c)
            gl.readPixels(0, 0, c.width, c.height, gl.RGBA, gl.UNSIGNED_BYTE, a)
            draw(optimized, c)
            gl.readPixels(0, 0, c.width, c.height, gl.RGBA, gl.UNSIGNED_BYTE, b)
            if (gl.isContextLost() || gl.getError() !== gl.NO_ERROR) throw Error('WebGL render/readback failed')
            if (a[3] !== 255 || b[3] !== 255) {
                state.alpha = { candidate: b[3], reference: a[3] }
                throw Error('The shaders did not produce opaque pixels')
            }
            let max = 0
            let unequal = 0
            for (let i = 0; i < a.length; i++) {
                const error = Math.abs(a[i] - b[i])
                max = Math.max(max, error)
                if (error) unequal++
            }
            maxChannelError = Math.max(maxChannelError, max)
            unequalChannels += unequal
            totalChannels += a.length
            if (max > 0) failures.push({ case: c, index, max, unequal })
            document.querySelector('pre').textContent = JSON.stringify({
                case: index + 1,
                maxChannelError,
                phase: 'parity',
                total: cases.length,
                unequalChannels,
            })
            await next()
        }
        return { cases: cases.length, failures, maxChannelError, totalChannels, unequalChannels }
    }
    async function measure(p, c) {
        if (!timer) return null
        const q = gl.createQuery()
        draw(p, c)
        gl.finish()
        // GPU elapsed queries measure submitted work; CPU readPixels is outside this window.
        gl.beginQuery(timer.TIME_ELAPSED_EXT, q)
        for (let i = 0; i < 8; i++) draw(p, { ...c, time: (c.time ?? defaults.time) + i / 60 })
        gl.endQuery(timer.TIME_ELAPSED_EXT)
        gl.flush()
        while (!gl.getQueryParameter(q, gl.QUERY_RESULT_AVAILABLE)) await next()
        if (gl.getParameter(timer.GPU_DISJOINT_EXT)) throw Error('Disjoint GPU timer')
        const ns = gl.getQueryParameter(q, gl.QUERY_RESULT)
        gl.deleteQuery(q)
        return ns / 8 / 1e6
    }
    async function benchmark() {
        if (!timer) return { unavailable: true }
        const results = []
        for (const c of [
            { count: 31.84, name: 'live32' },
            { count: 96, name: 'max96' },
            { count: 96, name: 'large96', size: 10 },
            { count: 96, name: 'small96', size: 1 },
            { count: 10, name: 'min10' },
        ]) {
            state.phase = 'benchmark'
            state.input = c
            for (let i = 0; i < 8; i++) {
                draw(baseline, c)
                draw(optimized, c)
            }
            gl.finish()
            const before = []
            const after = []
            for (let i = 0; i < 20; i++) {
                // Alternate order to reduce clock and thermal bias between the two shaders.
                if (i % 2) {
                    after.push(await measure(optimized, c))
                    before.push(await measure(baseline, c))
                } else {
                    before.push(await measure(baseline, c))
                    after.push(await measure(optimized, c))
                }
            }
            const median = (a) => a.toSorted((a, b) => a - b)[Math.floor(a.length / 2)]
            results.push({
                after,
                afterMs: median(after),
                before,
                beforeMs: median(before),
                case: c,
                speedup: median(before) / median(after),
            })
            document.querySelector('pre').textContent = JSON.stringify({ phase: 'benchmark', results })
        }
        return results
    }
    const pixels = await parity()
    return {
        antialias,
        benchmark: pixels.unequalChannels === 0 ? await benchmark() : null,
        contextAttributes: gl.getContextAttributes(),
        parity: pixels,
        renderer,
    }
}

function installRunner(run) {
    window.runCheck = async () => {
        document.querySelector('button').disabled = true
        let result
        try {
            result = await run()
        } catch (error) {
            result = { error: error.stack, state: window.gpuCheckState }
        }
        window.result = result
        document.querySelector('pre').textContent = JSON.stringify(result, null, 2)
        await fetch('/result', { body: JSON.stringify(result), method: 'POST' })
        return result
    }
    document.querySelector('button').addEventListener('click', window.runCheck)
}

const html = `<!doctype html><title>Bubble Garden GPU parity</title><button>Run GPU check</button><pre>Ready</pre><script>window.shaders=${JSON.stringify({ candidate, reference }).replaceAll('<', '\\u003c')};(${installRunner.toString()})(${harness.toString()});</script>`
const server = Bun.serve({
    async fetch(request) {
        const url = new URL(request.url)
        if (url.pathname === '/result' && request.method === 'POST') {
            const result = await request.json()
            console.log(JSON.stringify(result, null, 2))
            process.exitCode = !result.error && result.parity.unequalChannels === 0 ? 0 : 1
            server.stop()
            return new Response('Recorded')
        }
        return new Response(html, { headers: { 'content-type': 'text/html' } })
    },
    hostname: '127.0.0.1',
    port: Number(process.env.BUBBLE_GPU_PORT ?? 0),
})
console.log(server.url.href)
