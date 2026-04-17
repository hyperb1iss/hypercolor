export type TemplateKind = 'canvas' | 'shader' | 'face' | 'html'

export interface CliOutput {
    error(message: string): void
    log(message: string): void
}

export interface AddEffectOptions {
    audio: boolean
    editor?: string
    effectId: string
    output?: CliOutput
    template: TemplateKind
    workspaceDir: string
}

export interface AddEffectResult {
    createdPaths: string[]
    entryPath: string
    effectId: string
    template: TemplateKind
}

export interface ScaffoldWorkspaceOptions {
    audio: boolean
    firstEffectId: string
    install: boolean
    output?: CliOutput
    sdkPackageSpec: string
    targetDir: string
    template: TemplateKind
    workspaceName: string
    git: boolean
}

export interface ScaffoldWorkspaceResult {
    createdPaths: string[]
    firstEntryPath: string
    targetDir: string
    template: TemplateKind
    workspaceName: string
}

export interface PromptedAddEffectOptions {
    audio: boolean
    effectId: string
    template: TemplateKind
}

export interface PromptedScaffoldOptions extends PromptedAddEffectOptions {
    git: boolean
    install: boolean
    workspaceName: string
}

export interface ResolvedTemplates {
    createdPaths: string[]
    entryPath: string
}
