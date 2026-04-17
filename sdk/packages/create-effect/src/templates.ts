import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'

import { displayNameFromEffectId } from './naming'
import type { ResolvedTemplates, TemplateKind } from './types'

const PACKAGE_ROOT = resolve(import.meta.dirname, '..')
const TEMPLATES_ROOT = join(PACKAGE_ROOT, 'templates')

function renderTemplate(content: string, variables: Record<string, string>): string {
    let rendered = content
    for (const [key, value] of Object.entries(variables)) {
        rendered = rendered.replaceAll(`__${key}__`, value)
    }
    return rendered
}

function copyTemplateTree(
    sourceDir: string,
    targetDir: string,
    variables: Record<string, string>,
    createdPaths: string[],
): void {
    mkdirSync(targetDir, { recursive: true })

    for (const entry of readdirSync(sourceDir, { withFileTypes: true })) {
        const sourcePath = join(sourceDir, entry.name)
        const rawTargetName = entry.name.endsWith('.template') ? entry.name.slice(0, -'.template'.length) : entry.name
        const targetName = renderTemplate(rawTargetName, variables)
        const targetPath = join(targetDir, targetName)

        if (entry.isDirectory()) {
            copyTemplateTree(sourcePath, targetPath, variables, createdPaths)
            continue
        }

        const sourceStat = statSync(sourcePath)
        if (!sourceStat.isFile()) continue

        const content = readFileSync(sourcePath, 'utf8')
        writeFileSync(targetPath, renderTemplate(content, variables))
        createdPaths.push(targetPath)
    }
}

function effectTemplateDir(template: TemplateKind, audio: boolean): string {
    if (template === 'html') return join(TEMPLATES_ROOT, 'effects', 'html')
    return join(TEMPLATES_ROOT, 'effects', template, audio ? 'audio' : 'base')
}

function workspaceTemplateDir(template: TemplateKind): string {
    return join(TEMPLATES_ROOT, 'workspace', template === 'html' ? 'html' : 'ts')
}

function effectTargetDir(workspaceDir: string, template: TemplateKind, effectId: string): string {
    return template === 'html' ? join(workspaceDir, 'effects') : join(workspaceDir, 'effects', effectId)
}

export function effectEntryPath(workspaceDir: string, template: TemplateKind, effectId: string): string {
    return template === 'html'
        ? join(workspaceDir, 'effects', `${effectId}.html`)
        : join(workspaceDir, 'effects', effectId, 'main.ts')
}

export function createEffectFiles(args: {
    audio: boolean
    effectId: string
    template: TemplateKind
    workspaceDir: string
}): ResolvedTemplates {
    const { audio, effectId, template, workspaceDir } = args
    const targetDir = effectTargetDir(workspaceDir, template, effectId)
    const createdPaths: string[] = []

    const variables = {
        DISPLAY_NAME: displayNameFromEffectId(effectId),
        EFFECT_ID: effectId,
    }

    if (template === 'html') {
        const entryPath = effectEntryPath(workspaceDir, template, effectId)
        if (existsSync(entryPath)) {
            throw new Error(`Effect "${effectId}" already exists at ${entryPath}`)
        }
        mkdirSync(targetDir, { recursive: true })
    } else if (existsSync(targetDir)) {
        throw new Error(`Effect "${effectId}" already exists at ${targetDir}`)
    }

    copyTemplateTree(effectTemplateDir(template, audio), targetDir, variables, createdPaths)

    return {
        createdPaths,
        entryPath: effectEntryPath(workspaceDir, template, effectId),
    }
}

export function createWorkspaceFiles(args: {
    sdkPackageSpec: string
    template: TemplateKind
    workspaceDir: string
    workspaceName: string
}): string[] {
    const { sdkPackageSpec, template, workspaceDir, workspaceName } = args
    const createdPaths: string[] = []
    mkdirSync(workspaceDir, { recursive: true })

    copyTemplateTree(
        workspaceTemplateDir(template),
        workspaceDir,
        {
            SDK_PACKAGE_SPEC: sdkPackageSpec,
            WORKSPACE_NAME: workspaceName,
        },
        createdPaths,
    )

    return createdPaths
}
