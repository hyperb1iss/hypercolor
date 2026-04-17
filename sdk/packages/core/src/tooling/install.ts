import { existsSync, mkdirSync } from 'node:fs'
import { cp, readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, isAbsolute, join, resolve } from 'node:path'

import { validateHtmlArtifact } from './validate'
import type { InstallArtifactsOptions, InstallArtifactsResult } from './types'

function defaultUserEffectsDir(): string {
    const dataHome = process.env.XDG_DATA_HOME ?? join(homedir(), '.local', 'share')
    return join(dataHome, 'hypercolor', 'effects', 'user')
}

async function expandPattern(pattern: string, cwd: string): Promise<string[]> {
    if (!pattern.includes('*') && !pattern.includes('?') && !pattern.includes('[')) {
        const resolvedPattern = isAbsolute(pattern) ? pattern : resolve(cwd, pattern)
        return existsSync(resolvedPattern) ? [resolvedPattern] : []
    }

    const glob = new Bun.Glob(pattern)
    const matches: string[] = []
    for await (const match of glob.scan({ absolute: true, cwd })) {
        matches.push(match)
    }
    return matches.sort()
}

export async function resolveInstallInputs(filePatterns: string[] | undefined, cwd: string): Promise<string[]> {
    const patterns = filePatterns?.length ? filePatterns : ['dist/*.html']
    const files = new Set<string>()

    for (const pattern of patterns) {
        const matches = await expandPattern(pattern, cwd)
        for (const match of matches) files.add(match)
    }

    return Array.from(files).sort()
}

function dedupeDestinationPath(destinationDir: string, sourcePath: string): string {
    const baseName = basename(sourcePath)
    const ext = baseName.includes('.') ? baseName.slice(baseName.lastIndexOf('.')) : ''
    const stem = ext ? baseName.slice(0, -ext.length) : baseName

    let attempt = 0
    while (true) {
        const candidateName = attempt === 0 ? `${stem}${ext}` : `${stem}-${attempt + 1}${ext}`
        const candidate = join(destinationDir, candidateName)
        if (!existsSync(candidate)) return candidate
        attempt += 1
    }
}

export async function installArtifactsLocally(options: InstallArtifactsOptions = {}): Promise<InstallArtifactsResult> {
    const cwd = options.cwd ?? process.cwd()
    const destinationDir = options.userEffectsDir ?? defaultUserEffectsDir()
    const inputs = await resolveInstallInputs(options.filePatterns, cwd)
    const result: InstallArtifactsResult = { failures: [], successes: [] }

    mkdirSync(destinationDir, { recursive: true })

    for (const file of inputs) {
        const html = await readFile(file, 'utf8')
        const validation = validateHtmlArtifact(html, file)
        if (!validation.valid) {
            result.failures.push({
                errors: validation.errors.map((entry) => entry.message),
                file,
            })
            continue
        }

        const installedPath = dedupeDestinationPath(destinationDir, file)
        await cp(file, installedPath)
        result.successes.push({
            file,
            installedPath,
            warnings: validation.warnings,
        })
    }

    return result
}
