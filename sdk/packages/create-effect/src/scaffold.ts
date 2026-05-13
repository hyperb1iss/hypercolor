import { existsSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'

import { normalizeEffectId, normalizeWorkspaceName } from './naming'
import { createEffectFiles, createWorkspaceFiles } from './templates'
import type {
    AddEffectOptions,
    AddEffectResult,
    CliOutput,
    ScaffoldWorkspaceOptions,
    ScaffoldWorkspaceResult,
} from './types'

function output(consoleLike: CliOutput | undefined): CliOutput {
    return consoleLike ?? console
}

function assertDirectoryAvailable(targetDir: string): void {
    if (!existsSync(targetDir)) return
    if (readdirSync(targetDir).length === 0) return
    throw new Error(`Target directory already exists and is not empty: ${targetDir}`)
}

async function runCommand(cmd: string[], cwd: string, message: string): Promise<void> {
    const proc = Bun.spawn({
        cmd,
        cwd,
        stderr: 'inherit',
        stdin: 'ignore',
        stdout: 'inherit',
    })
    const exitCode = await proc.exited
    if (exitCode !== 0) {
        throw new Error(`${message} (exit ${exitCode})`)
    }
}

export async function addEffect(options: AddEffectOptions): Promise<AddEffectResult> {
    const effectId = normalizeEffectId(options.effectId)
    if (!effectId) throw new Error('Effect name must resolve to a non-empty id')

    const workspaceDir = resolve(options.workspaceDir)
    const created = createEffectFiles({
        audio: options.audio,
        effectId,
        template: options.template,
        workspaceDir,
    })

    const stdout = output(options.output)
    stdout.log(`Created ${effectId} (${options.template})`)

    const editor = options.editor?.trim()
    if (editor) {
        await runCommand([editor, created.entryPath], workspaceDir, `Failed to open ${created.entryPath}`)
    }

    return {
        createdPaths: created.createdPaths,
        effectId,
        entryPath: created.entryPath,
        template: options.template,
    }
}

export async function scaffoldWorkspace(options: ScaffoldWorkspaceOptions): Promise<ScaffoldWorkspaceResult> {
    const workspaceName = normalizeWorkspaceName(options.workspaceName)
    const firstEffectId = normalizeEffectId(options.firstEffectId)
    if (!workspaceName) throw new Error('Workspace name must not be empty')
    if (!firstEffectId) throw new Error('First effect name must resolve to a non-empty id')

    const targetDir = resolve(options.targetDir)
    assertDirectoryAvailable(targetDir)

    const createdPaths = createWorkspaceFiles({
        sdkPackageSpec: options.sdkPackageSpec,
        template: options.template,
        workspaceDir: targetDir,
        workspaceName,
    })
    const createdEffect = createEffectFiles({
        audio: options.audio,
        effectId: firstEffectId,
        template: options.template,
        workspaceDir: targetDir,
    })

    createdPaths.push(...createdEffect.createdPaths)

    if (options.git) {
        await runCommand(['git', 'init'], targetDir, 'Failed to initialize git repository')
    }

    if (options.install) {
        await runCommand(['bun', 'install'], targetDir, 'Failed to install workspace dependencies')
    }

    return {
        createdPaths,
        firstEntryPath: createdEffect.entryPath,
        targetDir,
        template: options.template,
        workspaceName,
    }
}

export function defaultSdkPackageSpec(): string | undefined {
    return process.env.HYPERCOLOR_SDK_PACKAGE_SPEC
}

export function resolveWorkspaceTarget(cwd: string, workspaceName: string): string {
    return resolve(cwd, workspaceName)
}

export function resolveEditor(): string | undefined {
    return process.env.VISUAL ?? process.env.EDITOR
}

export function workspaceNameFromTarget(target: string): string {
    return normalizeWorkspaceName(target.split('/').at(-1) ?? target)
}
