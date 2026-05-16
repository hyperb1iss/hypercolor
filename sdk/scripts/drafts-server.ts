#!/usr/bin/env bun
/**
 * Interactive drafts browser & picker.
 *
 * Serves a grid of every captured rank PNG grouped by effect/variant. Click
 * any thumb to mark it as the "top" for that variant; selections persist to
 * selections.json next to the drafts. The "Promote selected" button
 * re-encodes each picked frame into effects/screenshots/curated/<slug>/
 * <variant>.webp at quality 92 — the same path capture-screenshots --promote
 * uses, but driven by your picks instead of the rank-1 default.
 *
 * Usage: bun sdk/scripts/drafts-server.ts [--port 9431]
 */

import { mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'

import sharp from 'sharp'

const SDK_ROOT = resolve(import.meta.dirname, '..')
const REPO_ROOT = resolve(SDK_ROOT, '..')
const DRAFTS_ROOT = resolve(REPO_ROOT, 'effects', 'screenshots', 'drafts')
const CURATED_ROOT = resolve(REPO_ROOT, 'effects', 'screenshots', 'curated')
const SELECTIONS_PATH = resolve(DRAFTS_ROOT, 'selections.json')

interface RankEntry {
    rank: number
    path: string
}
interface VariantEntry {
    key: string
    ranks: RankEntry[]
}
interface EffectEntry {
    slug: string
    variants: VariantEntry[]
}
type Selections = Record<string, Record<string, number>>

interface CliOptions {
    port: number
}

function parseArgs(argv: readonly string[]): CliOptions {
    const opts: CliOptions = { port: 9431 }
    for (let index = 0; index < argv.length; index += 1) {
        const arg = argv[index]
        const next = argv[index + 1]
        if (arg === '--port') {
            if (!next) throw new Error('--port requires a number')
            opts.port = Number.parseInt(next, 10)
            index += 1
        } else if (arg === '-h' || arg === '--help') {
            process.stdout.write(`drafts-server — interactive picker for effect screenshot drafts

usage:
  bun sdk/scripts/drafts-server.ts [--port <n>]

defaults: port 9431
`)
            process.exit(0)
        } else {
            throw new Error(`unknown argument: ${arg}`)
        }
    }
    return opts
}

async function isDir(path: string): Promise<boolean> {
    try {
        return (await stat(path)).isDirectory()
    } catch {
        return false
    }
}

async function collectEffects(): Promise<EffectEntry[]> {
    const slugs = await readdir(DRAFTS_ROOT)
    const effects: EffectEntry[] = []
    for (const slug of slugs.sort()) {
        const effectDir = resolve(DRAFTS_ROOT, slug)
        if (!(await isDir(effectDir))) continue
        const variantNames = await readdir(effectDir)
        const variants: VariantEntry[] = []
        for (const name of variantNames.sort()) {
            const variantDir = resolve(effectDir, name)
            if (!(await isDir(variantDir))) continue
            const files = (await readdir(variantDir))
                .filter((f) => /^rank-\d+\.png$/.test(f))
                .sort()
            if (files.length === 0) continue
            variants.push({
                key: name,
                ranks: files.map((f) => {
                    const match = /^rank-(\d+)\.png$/.exec(f)
                    return {
                        path: resolve(variantDir, f),
                        rank: Number.parseInt(match?.[1] ?? '0', 10),
                    }
                }),
            })
        }
        if (variants.length > 0) effects.push({ slug, variants })
    }
    return effects
}

async function loadSelections(): Promise<Selections> {
    try {
        const raw = await readFile(SELECTIONS_PATH, 'utf8')
        return JSON.parse(raw) as Selections
    } catch {
        return {}
    }
}

async function saveSelections(selections: Selections): Promise<void> {
    await writeFile(SELECTIONS_PATH, `${JSON.stringify(selections, null, 2)}\n`, 'utf8')
}

async function promote(effects: EffectEntry[], selections: Selections): Promise<number> {
    let count = 0
    for (const effect of effects) {
        const variantPicks = selections[effect.slug]
        if (!variantPicks) continue
        for (const variant of effect.variants) {
            const picked = variantPicks[variant.key]
            if (!picked) continue
            const rankPath = variant.ranks.find((r) => r.rank === picked)?.path
            if (!rankPath) continue
            const outDir = resolve(CURATED_ROOT, effect.slug)
            await mkdir(outDir, { recursive: true })
            const outPath = resolve(outDir, `${variant.key}.webp`)
            const bytes = await readFile(rankPath)
            await sharp(bytes).webp({ effort: 4, quality: 92 }).toFile(outPath)
            count += 1
        }
    }
    return count
}

function escapeHtml(input: string): string {
    return input.replace(
        /[&<>"']/g,
        (ch) =>
            ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[ch] ?? ch,
    )
}

function renderHtml(effects: EffectEntry[]): string {
    const totalVariants = effects.reduce((sum, e) => sum + e.variants.length, 0)
    const toc = effects
        .map((e) => `<li><a href="#${escapeHtml(e.slug)}">${escapeHtml(e.slug)}</a></li>`)
        .join('\n')

    const sections = effects
        .map((effect) => {
            const variants = effect.variants
                .map((variant) => {
                    const ranks = variant.ranks
                        .map(
                            (entry) => `
                <figure class="rank" data-rank="${entry.rank}" role="button" tabindex="0" aria-label="rank ${entry.rank}">
                    <img src="/img/${escapeHtml(effect.slug)}/${escapeHtml(variant.key)}/rank-${entry.rank}.png" alt="${escapeHtml(`${effect.slug} ${variant.key} rank-${entry.rank}`)}" loading="lazy">
                    <figcaption>rank-${entry.rank}</figcaption>
                </figure>`,
                        )
                        .join('')
                    return `
        <section class="variant" data-slug="${escapeHtml(effect.slug)}" data-variant="${escapeHtml(variant.key)}">
            <header><h3>${escapeHtml(variant.key)}</h3></header>
            <div class="rank-row">${ranks}</div>
        </section>`
                })
                .join('')
            return `
<article class="effect" id="${escapeHtml(effect.slug)}">
    <header class="effect-head">
        <h2>${escapeHtml(effect.slug)}</h2>
        <span class="badge">${effect.variants.length} variant${effect.variants.length === 1 ? '' : 's'}</span>
    </header>
    ${variants}
</article>`
        })
        .join('\n')

    return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Hypercolor drafts</title>
<style>
:root {
    color-scheme: dark;
    --bg: #0a0612;
    --panel: #15102a;
    --border: rgba(225, 53, 255, 0.18);
    --accent: #e135ff;
    --cyan: #80ffea;
    --text: #f5edff;
    --muted: rgba(245, 237, 255, 0.55);
    --success: #50fa7b;
    --warning: #f1fa8c;
    --error: #ff6363;
}
* { box-sizing: border-box; }
body {
    margin: 0;
    background: var(--bg);
    color: var(--text);
    font: 14px/1.5 'JetBrains Mono', 'Fira Code', ui-monospace, monospace;
    padding: 24px 32px 80px;
}
header.page {
    position: sticky;
    top: 0;
    z-index: 10;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 24px;
    margin: -24px -32px 32px;
    padding: 20px 32px;
    background: rgba(10, 6, 18, 0.92);
    backdrop-filter: blur(12px);
    border-bottom: 1px solid var(--border);
}
header.page h1 {
    margin: 0 0 2px;
    font-size: 22px;
    color: var(--accent);
    letter-spacing: 0.04em;
}
header.page p { color: var(--muted); margin: 0; font-size: 12px; }
.toolbar { display: flex; gap: 12px; align-items: center; }
.status {
    color: var(--muted);
    font-size: 12px;
    padding: 6px 12px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    min-width: 140px;
    text-align: center;
}
.status.ok { color: var(--success); border-color: rgba(80, 250, 123, 0.4); }
.status.err { color: var(--error); border-color: rgba(255, 99, 99, 0.4); }
button.primary {
    background: linear-gradient(135deg, var(--accent), #80ffea);
    color: #0a0612;
    font-weight: 700;
    border: none;
    padding: 10px 22px;
    border-radius: 8px;
    cursor: pointer;
    font-family: inherit;
    font-size: 12px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    transition: opacity 0.15s, transform 0.1s;
}
button.primary:hover:not(:disabled) { transform: translateY(-1px); }
button.primary:disabled { opacity: 0.4; cursor: not-allowed; }
nav.toc {
    margin: 0 0 40px;
    padding: 16px 20px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
}
nav.toc h2 {
    font-size: 12px;
    margin: 0 0 12px;
    color: var(--cyan);
    text-transform: uppercase;
    letter-spacing: 0.12em;
}
nav.toc ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 4px 12px;
}
nav.toc a { color: var(--text); text-decoration: none; font-size: 13px; }
nav.toc a:hover { color: var(--accent); }
nav.toc li.has-pick a::before { content: '● '; color: var(--success); }
article.effect {
    margin-bottom: 48px;
    padding: 24px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 16px;
    scroll-margin-top: 96px;
}
.effect-head {
    display: flex;
    align-items: baseline;
    gap: 16px;
    margin-bottom: 12px;
    padding-bottom: 12px;
    border-bottom: 1px solid var(--border);
}
.effect-head h2 { margin: 0; font-size: 20px; color: var(--cyan); letter-spacing: 0.02em; }
.badge {
    color: var(--muted);
    font-size: 11px;
    padding: 2px 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
}
section.variant { margin: 16px 0; }
section.variant header h3 {
    font-size: 12px;
    margin: 0 0 10px;
    color: var(--accent);
    text-transform: uppercase;
    letter-spacing: 0.1em;
}
.rank-row {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    gap: 14px;
}
figure.rank {
    margin: 0;
    background: #050310;
    border: 2px solid transparent;
    border-radius: 12px;
    overflow: hidden;
    cursor: pointer;
    transition: border-color 0.15s, transform 0.1s, box-shadow 0.15s;
    position: relative;
}
figure.rank:hover {
    border-color: rgba(225, 53, 255, 0.4);
    transform: translateY(-2px);
}
figure.rank:focus-visible {
    outline: none;
    border-color: var(--cyan);
}
figure.rank.selected {
    border-color: var(--accent);
    box-shadow: 0 0 24px rgba(225, 53, 255, 0.4);
}
figure.rank.selected::after {
    content: 'TOP';
    position: absolute;
    top: 8px;
    right: 8px;
    background: var(--accent);
    color: #0a0612;
    font-size: 10px;
    font-weight: 700;
    padding: 4px 10px;
    border-radius: 4px;
    letter-spacing: 0.12em;
}
figure.rank img {
    display: block;
    width: 100%;
    height: auto;
    aspect-ratio: 5 / 4;
    object-fit: cover;
}
figure.rank figcaption {
    text-align: center;
    padding: 6px 0;
    color: var(--muted);
    font-size: 11px;
    letter-spacing: 0.08em;
}
</style>
</head>
<body>
<header class="page">
    <div>
        <h1>Hypercolor drafts</h1>
        <p>${effects.length} effects · ${totalVariants} variants · click any frame to set it as TOP</p>
    </div>
    <div class="toolbar">
        <span class="status" id="status">loading…</span>
        <button class="primary" id="promote-btn" disabled>Promote selected</button>
    </div>
</header>
<nav class="toc"><h2>Effects</h2><ul id="toc">${toc}</ul></nav>
${sections}
<script>
const statusEl = document.getElementById('status');
const promoteBtn = document.getElementById('promote-btn');
const tocLinks = new Map();
document.querySelectorAll('#toc li').forEach((li) => {
    const a = li.querySelector('a');
    if (a) tocLinks.set(a.getAttribute('href').slice(1), li);
});

let selections = {};

function setStatus(text, kind = '') {
    statusEl.textContent = text;
    statusEl.className = 'status' + (kind ? ' ' + kind : '');
}

function applyHighlights() {
    let count = 0;
    document.querySelectorAll('section.variant').forEach((section) => {
        const { slug, variant } = section.dataset;
        const picked = selections[slug] && selections[slug][variant];
        section.querySelectorAll('figure.rank').forEach((fig) => {
            const rank = Number(fig.dataset.rank);
            fig.classList.toggle('selected', picked === rank);
        });
        if (picked) count += 1;
    });
    tocLinks.forEach((li, slug) => {
        const hasPick = !!selections[slug] && Object.keys(selections[slug]).length > 0;
        li.classList.toggle('has-pick', hasPick);
    });
    promoteBtn.disabled = count === 0;
    setStatus(\`\${count} pick\${count === 1 ? '' : 's'}\`, count > 0 ? 'ok' : '');
}

async function loadSelections() {
    try {
        const res = await fetch('/api/selections');
        selections = await res.json();
        applyHighlights();
    } catch (err) {
        setStatus('failed to load picks', 'err');
    }
}

async function pickRank(slug, variant, rank) {
    const previous = selections[slug] && selections[slug][variant];
    selections[slug] = selections[slug] || {};
    if (previous === rank) {
        delete selections[slug][variant];
        if (Object.keys(selections[slug]).length === 0) delete selections[slug];
    } else {
        selections[slug][variant] = rank;
    }
    applyHighlights();
    try {
        await fetch('/api/selections', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ slug, variant, rank: previous === rank ? null : rank }),
        });
    } catch (err) {
        setStatus('save failed', 'err');
    }
}

document.querySelectorAll('section.variant').forEach((section) => {
    const { slug, variant } = section.dataset;
    section.querySelectorAll('figure.rank').forEach((fig) => {
        const rank = Number(fig.dataset.rank);
        const fire = () => pickRank(slug, variant, rank);
        fig.addEventListener('click', fire);
        fig.addEventListener('keydown', (event) => {
            if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                fire();
            }
        });
    });
});

promoteBtn.addEventListener('click', async () => {
    const count = Object.values(selections).reduce(
        (sum, variants) => sum + Object.keys(variants).length,
        0,
    );
    if (!confirm(\`Promote \${count} pick\${count === 1 ? '' : 's'} into curated/?\`)) return;
    promoteBtn.disabled = true;
    setStatus('promoting…');
    try {
        const res = await fetch('/api/promote', { method: 'POST' });
        const result = await res.json();
        if (result.ok) {
            setStatus(\`promoted \${result.promoted}\`, 'ok');
        } else {
            setStatus('promote failed', 'err');
        }
    } catch (err) {
        setStatus('promote failed', 'err');
    } finally {
        applyHighlights();
    }
});

loadSelections();
</script>
</body>
</html>
`
}

const VARIANT_PATH_RE = /^[a-z0-9][a-z0-9-]*\/[a-z0-9][a-z0-9-]*\/rank-\d+\.png$/i

async function main(): Promise<void> {
    const opts = parseArgs(process.argv.slice(2))
    const effects = await collectEffects()
    if (effects.length === 0) {
        process.stderr.write(`no drafts under ${DRAFTS_ROOT}\n`)
        process.exit(1)
    }

    const server = Bun.serve({
        port: opts.port,
        async fetch(req) {
            const url = new URL(req.url)
            const path = url.pathname

            if (path === '/') {
                return new Response(renderHtml(effects), {
                    headers: { 'Content-Type': 'text/html; charset=utf-8' },
                })
            }

            if (path === '/api/selections' && req.method === 'GET') {
                return Response.json(await loadSelections())
            }

            if (path === '/api/selections' && req.method === 'POST') {
                const body = (await req.json()) as {
                    slug: string
                    variant: string
                    rank: number | null
                }
                const current = await loadSelections()
                current[body.slug] = current[body.slug] ?? {}
                if (body.rank === null) {
                    delete current[body.slug][body.variant]
                    if (Object.keys(current[body.slug]).length === 0) delete current[body.slug]
                } else {
                    current[body.slug][body.variant] = body.rank
                }
                await saveSelections(current)
                return Response.json({ ok: true })
            }

            if (path === '/api/promote' && req.method === 'POST') {
                try {
                    const current = await loadSelections()
                    const promoted = await promote(effects, current)
                    return Response.json({ ok: true, promoted })
                } catch (err) {
                    return Response.json(
                        { ok: false, error: String(err) },
                        { status: 500 },
                    )
                }
            }

            if (path.startsWith('/img/')) {
                const rel = path.slice(5)
                if (!VARIANT_PATH_RE.test(rel)) {
                    return new Response('bad path', { status: 400 })
                }
                const imgPath = resolve(DRAFTS_ROOT, rel)
                if (!imgPath.startsWith(`${DRAFTS_ROOT}/`)) {
                    return new Response('forbidden', { status: 403 })
                }
                return new Response(Bun.file(imgPath))
            }

            return new Response('not found', { status: 404 })
        },
    })

    process.stdout.write(`drafts server: http://localhost:${server.port}\n`)
    process.stdout.write(`drafts root:    ${DRAFTS_ROOT}\n`)
    process.stdout.write(`selections at:  ${SELECTIONS_PATH}\n`)
    process.stdout.write(`click a thumb to pick TOP, then hit "Promote selected"\n`)
}

await main()
