import { cancel, confirm, intro, isCancel, select, text } from '@clack/prompts'

import { isTemplateKind, normalizeEffectId, normalizeWorkspaceName } from './naming'
import type { PromptedAddEffectOptions, PromptedScaffoldOptions, TemplateKind } from './types'

function unwrapPrompt<T>(value: T | symbol): T {
    if (!isCancel(value)) return value as T
    cancel('Scaffolding canceled.')
    throw new Error('Prompt canceled')
}

function templateOptions() {
    return [
        { label: 'Canvas', value: 'canvas' },
        { label: 'Shader', value: 'shader' },
        { label: 'Face', value: 'face' },
        { label: 'HTML', value: 'html' },
    ] satisfies Array<{ label: string; value: TemplateKind }>
}

async function promptEffectId(initialValue: string | undefined, label: string): Promise<string> {
    if (initialValue) return initialValue

    const value = unwrapPrompt(
        await text({
            message: label,
            placeholder: 'my-effect',
            validate(input) {
                return normalizeEffectId(input).length > 0 ? undefined : 'Enter an effect name'
            },
        }),
    )

    return normalizeEffectId(String(value))
}

async function promptTemplate(initialValue: TemplateKind | undefined): Promise<TemplateKind> {
    if (initialValue) return initialValue
    return unwrapPrompt(
        await select({
            message: 'Pick a starter template',
            options: templateOptions(),
        }),
    )
}

export async function promptAddEffectOptions(initial: {
    audio?: boolean
    effectId?: string
    template?: string
}): Promise<PromptedAddEffectOptions> {
    intro('Hypercolor effect starter')

    const templateCandidate = initial.template
    const seededTemplate: TemplateKind | undefined =
        templateCandidate && isTemplateKind(templateCandidate) ? templateCandidate : undefined
    const template = seededTemplate ?? (await promptTemplate(undefined))
    const effectId = await promptEffectId(initial.effectId, 'Name for your effect')
    const audio =
        initial.audio ??
        unwrapPrompt(
            await confirm({
                initialValue: false,
                message: 'Audio reactive?',
            }),
        )

    return { audio, effectId, template }
}

export async function promptWorkspaceOptions(initial: {
    audio?: boolean
    effectId?: string
    git?: boolean
    install?: boolean
    template?: string
    workspaceName?: string
}): Promise<PromptedScaffoldOptions> {
    intro('Hypercolor workspace starter')

    const workspaceName =
        initial.workspaceName ??
        normalizeWorkspaceName(
            String(
                unwrapPrompt(
                    await text({
                        message: "What's your workspace called?",
                        placeholder: 'my-effects',
                        validate(input) {
                            return normalizeWorkspaceName(input).length > 0 ? undefined : 'Enter a workspace name'
                        },
                    }),
                ),
            ),
        )
    const templateCandidate = initial.template
    const seededTemplate: TemplateKind | undefined =
        templateCandidate && isTemplateKind(templateCandidate) ? templateCandidate : undefined
    const template = seededTemplate ?? (await promptTemplate(undefined))
    const effectId = await promptEffectId(initial.effectId, 'Name for your first effect')
    const audio =
        initial.audio ??
        unwrapPrompt(
            await confirm({
                initialValue: false,
                message: 'Audio reactive?',
            }),
        )
    return {
        audio,
        effectId,
        git: initial.git ?? true,
        install: initial.install ?? true,
        template,
        workspaceName,
    }
}
