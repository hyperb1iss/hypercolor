export type ArtifactKind = 'effect' | 'face'

export interface PresetDef {
    name: string
    description?: string
    controls: Record<string, unknown>
}

export interface ResolvedControlSpec {
    __type: string
    label: string
    defaultValue: unknown
    tooltip?: string
    group?: string
    meta: Record<string, unknown>
}

export interface ResolvedControlDef {
    key: string
    spec: ResolvedControlSpec
    uniformName?: string
}

export interface RegisteredArtifactDef {
    type?: 'canvas' | 'webgl' | 'face'
    name: string
    shader?: string
    description?: string
    author?: string
    audio?: boolean
    screen?: boolean
    category?: string
    builtinId?: string
    presets?: PresetDef[]
    controls: Record<string, unknown>
    resolvedControls: ResolvedControlDef[]
}

export interface BuildControlDef {
    id: string
    type: string
    label?: string
    tooltip?: string
    group?: string
    default?: unknown
    min?: number
    max?: number
    values?: string[]
    step?: number
    aspectLock?: number
    preview?: 'screen' | 'web' | 'canvas'
}

export interface ExtractedArtifactMetadata {
    kind: ArtifactKind
    name: string
    description: string
    author: string
    audioReactive: boolean
    screenReactive: boolean
    category?: string
    builtinId?: string
    renderer?: string
    controls: BuildControlDef[]
    presets: PresetDef[]
}

export interface BuildArtifactResult {
    bytes: number
    entryPath: string
    html: string
    id: string
    kind: ArtifactKind
    metadata: ExtractedArtifactMetadata
    outputPath: string
}

export interface BuildArtifactsOptions {
    entryPaths: string[]
    outDir: string
    minify?: boolean
    sdkAliasPath?: string
}

export interface HtmlControlMetadata {
    property: string
    label: string
    type: string
    defaultValue?: string
    min?: number
    max?: number
    step?: number
    values: string[]
    tooltip?: string
    group?: string
}

export interface HtmlPresetMetadata {
    name: string
    description?: string
    controls: Record<string, string>
    parseError?: string
}

export interface ParsedHtmlArtifact {
    audioReactive?: string
    canvasHeight?: number
    canvasWidth?: number
    controls: HtmlControlMetadata[]
    hasExternalAssets: boolean
    hasRenderSurface: boolean
    hasScript: boolean
    isFace: boolean
    presets: HtmlPresetMetadata[]
    publisher?: string
    title?: string
    version?: string
    description?: string
}

export interface ValidationMessage {
    check: string
    code: string
    message: string
}

export interface ValidationResult {
    errors: ValidationMessage[]
    file: string
    metadata: {
        audioReactive: boolean
        controls: number
        presets: number
        title: string
    }
    valid: boolean
    warnings: ValidationMessage[]
}

export interface InstallArtifactSuccess {
    file: string
    installedPath: string
    warnings: ValidationMessage[]
}

export interface InstallArtifactFailure {
    errors: string[]
    file: string
}

export interface InstallArtifactsOptions {
    cwd?: string
    filePatterns?: string[]
    userEffectsDir?: string
}

export interface InstallArtifactsResult {
    failures: InstallArtifactFailure[]
    successes: InstallArtifactSuccess[]
}
