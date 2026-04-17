import type { HtmlControlMetadata, HtmlPresetMetadata, ParsedHtmlArtifact } from './types'

function stripHtmlComments(html: string): string {
    return html.replaceAll(/<!--[\s\S]*?-->/g, '')
}

function extractStartTags(html: string, tagName: string): string[] {
    return Array.from(html.matchAll(new RegExp(`<${tagName}\\b[^>]*>`, 'gi')), (match) => match[0])
}

function parseTagAttributes(tag: string): Map<string, string> {
    const attrs = new Map<string, string>()

    for (const match of tag.matchAll(/([:@\w-]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g)) {
        const [, rawName, doubleQuoted, singleQuoted, bare] = match
        if (!rawName || rawName.startsWith('<')) continue
        attrs.set(rawName.toLowerCase(), doubleQuoted ?? singleQuoted ?? bare ?? '')
    }

    return attrs
}

function attr(attrs: Map<string, string>, name: string): string | undefined {
    return attrs.get(name.toLowerCase())
}

function extractTitle(html: string): string | undefined {
    const match = html.match(/<title[^>]*>([\s\S]*?)<\/title>/i)
    return match?.[1]?.trim() || undefined
}

function parseNumber(value: string | undefined): number | undefined {
    if (!value) return undefined

    const number = Number(value)
    return Number.isFinite(number) ? number : undefined
}

function parseControl(attrs: Map<string, string>): HtmlControlMetadata | undefined {
    const property = attr(attrs, 'property')
    if (!property) return undefined

    return {
        defaultValue: attr(attrs, 'default'),
        group: attr(attrs, 'group'),
        label: attr(attrs, 'label') ?? property,
        max: parseNumber(attr(attrs, 'max')),
        min: parseNumber(attr(attrs, 'min')),
        property,
        step: parseNumber(attr(attrs, 'step')),
        tooltip: attr(attrs, 'tooltip'),
        type: (attr(attrs, 'type') ?? 'number').toLowerCase(),
        values: (attr(attrs, 'values') ?? '')
            .split(',')
            .map((value) => value.trim())
            .filter(Boolean),
    }
}

function parsePreset(attrs: Map<string, string>): HtmlPresetMetadata | undefined {
    const name = attr(attrs, 'preset')
    if (!name) return undefined

    const rawControls = attr(attrs, 'preset-controls')
    if (!rawControls) {
        return {
            controls: {},
            name,
            parseError: 'Missing preset-controls attribute',
        }
    }

    try {
        const parsed = JSON.parse(rawControls) as Record<string, unknown>
        const controls = Object.fromEntries(
            Object.entries(parsed).map(([key, value]) => [key, typeof value === 'string' ? value : String(value)]),
        )

        return {
            controls,
            description: attr(attrs, 'preset-description'),
            name,
        }
    } catch (error) {
        return {
            controls: {},
            description: attr(attrs, 'preset-description'),
            name,
            parseError: error instanceof Error ? error.message : String(error),
        }
    }
}

function canvasAttributes(html: string): { height?: number; width?: number } {
    const canvasTag = extractStartTags(html, 'canvas').find(
        (tag) => attr(parseTagAttributes(tag), 'id')?.toLowerCase() === 'excanvas',
    )
    if (!canvasTag) return {}

    const attrs = parseTagAttributes(canvasTag)
    return {
        height: parseNumber(attr(attrs, 'height')),
        width: parseNumber(attr(attrs, 'width')),
    }
}

export function parseHtmlArtifact(html: string): ParsedHtmlArtifact {
    const sanitized = stripHtmlComments(html)
    const metaTags = extractStartTags(sanitized, 'meta')
    const scripts = extractStartTags(sanitized, 'script')
    const links = extractStartTags(sanitized, 'link')
    const controls: HtmlControlMetadata[] = []
    const presets: HtmlPresetMetadata[] = []

    let description: string | undefined
    let publisher: string | undefined
    let version: string | undefined
    let audioReactive: string | undefined

    for (const tag of metaTags) {
        const attrs = parseTagAttributes(tag)

        const preset = parsePreset(attrs)
        if (preset) {
            presets.push(preset)
            continue
        }

        const control = parseControl(attrs)
        if (control) {
            controls.push(control)
            continue
        }

        description ??= attr(attrs, 'description')
        publisher ??= attr(attrs, 'publisher') ?? (attr(attrs, 'name') === 'author' ? attr(attrs, 'content') : undefined)
        version ??=
            (attr(attrs, 'name') === 'hypercolor-version' ? attr(attrs, 'content') : undefined) ??
            attr(attrs, 'hypercolor-version')
        audioReactive ??= attr(attrs, 'audio-reactive')
    }

    const hasExCanvas = /<canvas\b[^>]*id=["']exCanvas["'][^>]*>/i.test(sanitized)
    const hasFaceContainer = /<div\b[^>]*id=["']faceContainer["'][^>]*>/i.test(sanitized)
    const { height, width } = canvasAttributes(sanitized)

    return {
        audioReactive,
        canvasHeight: height,
        canvasWidth: width,
        controls,
        description,
        hasExternalAssets:
            scripts.some((tag) => Boolean(attr(parseTagAttributes(tag), 'src'))) ||
            links.some((tag) => Boolean(attr(parseTagAttributes(tag), 'href'))),
        hasRenderSurface: hasExCanvas || hasFaceContainer,
        hasScript: scripts.length > 0,
        isFace: hasFaceContainer,
        presets,
        publisher,
        title: extractTitle(sanitized),
        version,
    }
}
