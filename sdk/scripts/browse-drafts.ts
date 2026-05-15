#!/usr/bin/env bun
/**
 * Generate a static HTML browser for effect screenshot drafts.
 *
 * Walks effects/screenshots/drafts/ and renders a single index.html that lists
 * every effect/variant with its rank-1/2/3 frames side by side. Open the
 * resulting file in a browser to flip through and pick the best frame per
 * variant.
 *
 * Output: effects/screenshots/drafts/index.html (relative image paths).
 */

import { readdir, stat, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'

const SDK_ROOT = resolve(import.meta.dirname, '..')
const REPO_ROOT = resolve(SDK_ROOT, '..')
const DRAFTS_ROOT = resolve(REPO_ROOT, 'effects', 'screenshots', 'drafts')
const OUTPUT_PATH = resolve(REPO_ROOT, 'drafts-browser.html')

interface VariantEntry {
    key: string
    ranks: string[]
}

interface EffectEntry {
    slug: string
    variants: VariantEntry[]
}

async function isDir(path: string): Promise<boolean> {
    try {
        return (await stat(path)).isDirectory()
    } catch {
        return false
    }
}

async function listVariants(effectDir: string): Promise<VariantEntry[]> {
    const names = await readdir(effectDir)
    const variants: VariantEntry[] = []
    for (const name of names.sort()) {
        const variantDir = resolve(effectDir, name)
        if (!(await isDir(variantDir))) continue
        const files = (await readdir(variantDir))
            .filter((f) => /^rank-\d+\.png$/.test(f))
            .sort()
        if (files.length === 0) continue
        variants.push({
            key: name,
            ranks: files.map((f) => resolve(variantDir, f)),
        })
    }
    return variants
}

async function collectEffects(): Promise<EffectEntry[]> {
    const slugs = await readdir(DRAFTS_ROOT)
    const effects: EffectEntry[] = []
    for (const slug of slugs.sort()) {
        const effectDir = resolve(DRAFTS_ROOT, slug)
        if (!(await isDir(effectDir))) continue
        const variants = await listVariants(effectDir)
        if (variants.length > 0) effects.push({ slug, variants })
    }
    return effects
}

function escapeHtml(input: string): string {
    return input.replace(
        /[&<>"']/g,
        (ch) =>
            ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[ch] ?? ch,
    )
}

function renderHtml(effects: EffectEntry[]): string {
    const variantCount = effects.reduce((sum, e) => sum + e.variants.length, 0)
    const totalEffects = effects.length
    const sections = effects
        .map((effect) => {
            const variants = effect.variants
                .map((variant) => {
                    const ranks = variant.ranks
                        .map((absPath, index) => {
                            const url = `file://${absPath}`
                            return `
        <figure class="rank">
            <a href="${escapeHtml(url)}" target="_blank" rel="noreferrer">
                <img src="${escapeHtml(url)}" alt="${escapeHtml(`${effect.slug} ${variant.key} rank-${index + 1}`)}" loading="lazy">
            </a>
            <figcaption>rank-${index + 1}</figcaption>
        </figure>`
                        })
                        .join('')
                    return `
    <section class="variant" id="${escapeHtml(`${effect.slug}--${variant.key}`)}">
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

    const toc = effects
        .map((e) => `<li><a href="#${escapeHtml(e.slug)}">${escapeHtml(e.slug)}</a></li>`)
        .join('\n')

    return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Hypercolor effect drafts</title>
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
}
* { box-sizing: border-box; }
body {
    margin: 0;
    background: var(--bg);
    color: var(--text);
    font: 14px/1.5 'JetBrains Mono', 'Fira Code', ui-monospace, monospace;
    padding: 32px 40px 80px;
}
header.page {
    margin-bottom: 40px;
    border-bottom: 1px solid var(--border);
    padding-bottom: 24px;
}
header.page h1 {
    margin: 0 0 8px;
    font-size: 28px;
    letter-spacing: 0.04em;
    color: var(--accent);
}
header.page p { color: var(--muted); margin: 0; }
nav.toc {
    margin: 24px 0 48px;
    padding: 16px 20px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
}
nav.toc h2 {
    font-size: 13px;
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
nav.toc a {
    color: var(--text);
    text-decoration: none;
    font-size: 13px;
}
nav.toc a:hover { color: var(--accent); }
article.effect {
    margin-bottom: 56px;
    padding: 24px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 16px;
}
.effect-head {
    display: flex;
    align-items: baseline;
    gap: 16px;
    margin-bottom: 16px;
    padding-bottom: 12px;
    border-bottom: 1px solid var(--border);
}
.effect-head h2 {
    margin: 0;
    font-size: 22px;
    color: var(--cyan);
    letter-spacing: 0.02em;
}
.badge {
    color: var(--muted);
    font-size: 12px;
    padding: 2px 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
}
section.variant { margin: 20px 0; }
section.variant header h3 {
    font-size: 14px;
    margin: 0 0 12px;
    color: var(--accent);
    text-transform: uppercase;
    letter-spacing: 0.1em;
}
.rank-row {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 16px;
}
figure.rank {
    margin: 0;
    background: #050310;
    border: 1px solid var(--border);
    border-radius: 12px;
    overflow: hidden;
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
    padding: 8px 0;
    color: var(--muted);
    font-size: 12px;
    letter-spacing: 0.08em;
}
figure.rank a { display: block; }
</style>
</head>
<body>
<header class="page">
    <h1>Hypercolor effect drafts</h1>
    <p>${totalEffects} effect${totalEffects === 1 ? '' : 's'} · ${variantCount} variant${variantCount === 1 ? '' : 's'} · click a thumbnail to open the full PNG</p>
</header>
<nav class="toc">
    <h2>Effects</h2>
    <ul>${toc}</ul>
</nav>
${sections}
</body>
</html>
`
}

async function main(): Promise<void> {
    const effects = await collectEffects()
    if (effects.length === 0) {
        process.stderr.write(`no drafts found under ${DRAFTS_ROOT}\n`)
        process.exit(1)
    }
    const html = renderHtml(effects)
    await writeFile(OUTPUT_PATH, html, 'utf8')
    process.stdout.write(`wrote ${OUTPUT_PATH}\n`)
    process.stdout.write(`open file://${OUTPUT_PATH} in a browser to browse drafts\n`)
}

await main()
