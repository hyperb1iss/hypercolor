export function normalizeWorkspaceName(input: string): string {
    return input.trim()
}

export function normalizeEffectId(input: string): string {
    return input
        .trim()
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/^-+|-+$/g, '')
}

export function displayNameFromEffectId(effectId: string): string {
    return effectId
        .split('-')
        .filter(Boolean)
        .map((segment) => segment.charAt(0).toUpperCase() + segment.slice(1))
        .join(' ')
}

export function isTemplateKind(value: string): value is import('./types').TemplateKind {
    return value === 'canvas' || value === 'shader' || value === 'face' || value === 'html'
}
