import { existsSync, mkdirSync } from 'node:fs'
import { cp, readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, isAbsolute, join, resolve } from 'node:path'
import type { InstallArtifactsOptions, InstallArtifactsResult } from './types'
import { validateHtmlArtifact } from './validate'

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
            source: 'local',
            warnings: validation.warnings,
        })
    }

    return result
}

function daemonInstallUrl(baseUrl: string): string {
    return new URL('/api/v1/effects/install', baseUrl).toString()
}

function daemonBaseUrl(options: InstallArtifactsOptions): string {
    return options.daemonUrl ?? process.env.HYPERCOLOR_DAEMON_URL ?? 'http://127.0.0.1:9420'
}

export async function installArtifactsViaDaemon(
    options: InstallArtifactsOptions = {},
): Promise<InstallArtifactsResult> {
    const cwd = options.cwd ?? process.cwd()
    const inputs = await resolveInstallInputs(options.filePatterns, cwd)
    const result: InstallArtifactsResult = { failures: [], successes: [] }
    const url = daemonInstallUrl(daemonBaseUrl(options))

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

        const formData = new FormData()
        formData.append('file', new Blob([html], { type: 'text/html' }), basename(file))

        try {
            const response = await fetch(url, {
                body: formData,
                method: 'POST',
            })

            if (!response.ok) {
                let message = `Daemon install failed with HTTP ${response.status}`
                try {
                    const payload = (await response.json()) as {
                        error?: {
                            message?: string
                            details?: {
                                errors?: string[]
                            }
                        }
                    }
                    const detailErrors = payload.error?.details?.errors
                    if (detailErrors?.length) {
                        message = detailErrors.join('; ')
                    } else if (payload.error?.message) {
                        message = payload.error.message
                    }
                } catch {
                    // Ignore JSON parse failures and surface the HTTP status instead.
                }

                result.failures.push({ errors: [message], file })
                continue
            }

            const payload = (await response.json()) as {
                data: {
                    controls: number
                    name: string
                    path: string
                    presets: number
                }
            }
            result.successes.push({
                controls: payload.data.controls,
                file,
                installedName: payload.data.name,
                installedPath: payload.data.path,
                presets: payload.data.presets,
                source: 'daemon',
                warnings: validation.warnings,
            })
        } catch (error) {
            result.failures.push({
                errors: [error instanceof Error ? error.message : String(error)],
                file,
            })
        }
    }

    return result
}
