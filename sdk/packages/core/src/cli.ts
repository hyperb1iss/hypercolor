import { existsSync, watch } from 'node:fs'
import { dirname, resolve } from 'node:path'

import { addEffect, promptAddEffectOptions } from '@hypercolor/create-effect'
import type { TemplateKind } from '@hypercolor/create-effect'

import { runDevServer } from './dev'
import {
    buildArtifacts,
    discoverWorkspaceEntries,
    installArtifactsLocally,
    installArtifactsViaDaemon,
    validateHtmlArtifactFile,
} from './tooling'

interface CliContext {
    cwd: string
    stdout: Pick<Console, 'error' | 'log'>
}

type CommandHandler = (args: string[], context: CliContext) => Promise<number>

function optionValue(args: string[], name: string): string | undefined {
    const index = args.indexOf(name)
    if (index === -1) return undefined
    return args[index + 1]
}

function takeRepeatedValues(args: string[], name: string): string[] {
    const values: string[] = []
    for (let index = 0; index < args.length; index += 1) {
        if (args[index] === name && args[index + 1]) {
            values.push(args[index + 1]!)
        }
    }
    return values
}

function positionalArgs(args: string[]): string[] {
    const positionals: string[] = []
    for (let index = 0; index < args.length; index += 1) {
        const arg = args[index]
        if (!arg) continue
        if (arg.startsWith('--')) {
            const next = args[index + 1]
            if (next && !next.startsWith('--')) index += 1
            continue
        }
        positionals.push(arg)
    }
    return positionals
}

async function runBuild(args: string[], context: CliContext): Promise<number> {
    const workspaceRoot = resolve(context.cwd, optionValue(args, '--workspace-root') ?? '.')
    const outDir = resolve(context.cwd, optionValue(args, '--out') ?? 'dist')
    const entryRoots = takeRepeatedValues(args, '--entry-root')
    const sdkAliasPath = optionValue(args, '--sdk-alias-path')
        ? resolve(context.cwd, optionValue(args, '--sdk-alias-path')!)
        : undefined
    const minify = args.includes('--minify')
    const watchMode = args.includes('--watch')
    const buildAll = args.includes('--all') || positionalArgs(args).length === 0
    const explicitEntries = positionalArgs(args).map((entry) => resolve(context.cwd, entry))
    const roots = entryRoots.length > 0 ? entryRoots : ['effects']

    const buildOnce = async (): Promise<number> => {
        const entryPaths = buildAll ? discoverWorkspaceEntries(workspaceRoot, roots) : explicitEntries
        if (entryPaths.length === 0) {
            context.stdout.error('No effect entrypoints found to build.')
            return 1
        }

        const results = await buildArtifacts({
            entryPaths,
            minify,
            outDir,
            sdkAliasPath,
        })

        for (const result of results) {
            const icon = result.kind === 'face' ? '💎' : '✓'
            const sizeKB = (result.bytes / 1024).toFixed(1)
            context.stdout.log(`${icon} ${result.id} → ${result.outputPath} (${sizeKB} KB)`)
        }

        return 0
    }

    const initialExit = await buildOnce()
    if (initialExit !== 0 || !watchMode) return initialExit

    const watchRoots = buildAll
        ? roots.map((root) => resolve(workspaceRoot, root)).filter(existsSync)
        : Array.from(new Set(explicitEntries.map((entry) => dirname(entry))))
    const watchers = watchRoots.map((root) =>
        watch(root, { recursive: true }, (_eventType, filename) => {
            const next = String(filename ?? '')
            if (!next.endsWith('.ts') && !next.endsWith('.glsl')) return
            context.stdout.log(`↻ ${next}`)
            void buildOnce()
        }),
    )

    await new Promise<void>((resolveWatch) => {
        process.on('SIGINT', () => {
            for (const watcher of watchers) watcher.close()
            resolveWatch()
        })
    })

    return 0
}

function printHumanValidation(result: Awaited<ReturnType<typeof validateHtmlArtifactFile>>, context: CliContext): void {
    context.stdout.log(`\n${result.file}\n`)
    context.stdout.log(result.valid ? 'PASS  Render surface + title + script' : 'FAIL  Validation errors present')
    for (const warning of result.warnings) {
        context.stdout.log(`WARN  ${warning.message}`)
    }
    for (const error of result.errors) {
        context.stdout.log(`FAIL  ${error.message}`)
    }
    const suffix =
        result.warnings.length > 0
            ? ` (${result.warnings.length} warning${result.warnings.length === 1 ? '' : 's'})`
            : ''
    context.stdout.log(`\nResult: ${result.valid ? 'PASS' : 'FAIL'}${suffix}`)
}

async function runValidate(args: string[], context: CliContext): Promise<number> {
    const strict = args.includes('--strict')
    const json = args.includes('--json')
    const files = positionalArgs(args).map((file) => resolve(context.cwd, file))
    if (files.length === 0) {
        context.stdout.error('Usage: hypercolor validate <file.html> [more files...] [--strict] [--json]')
        return 1
    }

    const results = await Promise.all(files.map((file) => validateHtmlArtifactFile(file)))
    const hasErrors = results.some((result) => !result.valid)
    const hasWarnings = results.some((result) => result.warnings.length > 0)

    if (json) {
        context.stdout.log(JSON.stringify(results.length === 1 ? results[0] : results, null, 2))
    } else {
        for (const result of results) printHumanValidation(result, context)
    }

    return hasErrors || (strict && hasWarnings) ? 1 : 0
}

async function runInstall(args: string[], context: CliContext): Promise<number> {
    const daemonMode = args.includes('--daemon')
    const result = await (daemonMode ? installArtifactsViaDaemon : installArtifactsLocally)({
        cwd: context.cwd,
        daemonUrl: optionValue(args, '--daemon-url'),
        filePatterns: positionalArgs(args),
    })

    for (const success of result.successes) {
        const summary =
            success.source === 'daemon' && success.installedName
                ? `✓ ${success.file} → ${success.installedPath} (${success.installedName}, ${success.controls ?? 0} controls)`
                : `✓ ${success.file} → ${success.installedPath}`
        context.stdout.log(summary)
        for (const warning of success.warnings) {
            context.stdout.log(`  WARN  ${warning.message}`)
        }
    }

    for (const failure of result.failures) {
        context.stdout.error(`✗ ${failure.file}`)
        for (const message of failure.errors) {
            context.stdout.error(`  ${message}`)
        }
    }

    return result.failures.length > 0 || result.successes.length === 0 ? 1 : 0
}

async function runAdd(args: string[], context: CliContext): Promise<number> {
    const [name] = positionalArgs(args)
    const templateArg = optionValue(args, '--template')
    const prompted =
        name && templateArg
            ? undefined
            : await promptAddEffectOptions({
                  audio: args.includes('--audio') ? true : undefined,
                  effectId: name,
                  template: templateArg,
              })

    const effectId = prompted?.effectId ?? name
    const template: TemplateKind | undefined =
        prompted?.template ??
        (templateArg === 'canvas' || templateArg === 'shader' || templateArg === 'face' || templateArg === 'html'
            ? templateArg
            : undefined)
    const audio = prompted?.audio ?? args.includes('--audio')

    if (!effectId || !template) {
        context.stdout.error('Usage: hypercolor add [name] [--template canvas|shader|face|html] [--audio]')
        return 1
    }

    const result = await addEffect({
        audio,
        editor: [process.env.VISUAL, process.env.EDITOR].find((value) => value?.trim()),
        effectId,
        output: context.stdout,
        template,
        workspaceDir: context.cwd,
    })

    context.stdout.log(`Entry: ${result.entryPath}`)
    return 0
}

async function runDev(args: string[], context: CliContext): Promise<number> {
    const [entryArg] = positionalArgs(args)
    const workspaceRoot = resolve(context.cwd, optionValue(args, '--workspace-root') ?? '.')
    const entryRoots = takeRepeatedValues(args, '--entry-root')
    const sdkAliasPath = optionValue(args, '--sdk-alias-path')
        ? resolve(context.cwd, optionValue(args, '--sdk-alias-path')!)
        : undefined
    const port = Number.parseInt(optionValue(args, '--port') ?? '4200', 10)

    await runDevServer({
        cwd: context.cwd,
        entryPath: entryArg ? resolve(context.cwd, entryArg) : undefined,
        entryRoots: entryRoots.length > 0 ? entryRoots : ['effects'],
        open: args.includes('--open'),
        port: Number.isFinite(port) ? port : 4200,
        sdkAliasPath,
        stdout: context.stdout,
        workspaceRoot,
    })

    return 0
}

const NOT_IMPLEMENTED = new Set<string>()

const COMMANDS = new Map<string, CommandHandler>([
    ['add', runAdd],
    ['build', runBuild],
    ['dev', runDev],
    ['install', runInstall],
    ['validate', runValidate],
])

function printHelp(context: CliContext): void {
    context.stdout.log(`hypercolor <command>

Commands:
  dev        Start the Bun preview server
  build      Build effect entrypoints into HTML artifacts
  validate   Validate built HTML artifacts
  install    Install HTML artifacts into the user effects directory
  add        Scaffold a new effect inside the workspace
`)
}

export async function main(
    argv = process.argv.slice(2),
    context: CliContext = { cwd: process.cwd(), stdout: console },
): Promise<number> {
    const [command, ...args] = argv
    if (!command || command === '--help' || command === 'help') {
        printHelp(context)
        return 0
    }

    if (NOT_IMPLEMENTED.has(command)) {
        context.stdout.error(`The "${command}" command is not implemented yet.`)
        return 1
    }

    const handler = COMMANDS.get(command)
    if (!handler) {
        context.stdout.error(`Unknown command "${command}".`)
        printHelp(context)
        return 1
    }

    return handler(args, context)
}

if (import.meta.main) {
    const exitCode = await main()
    process.exit(exitCode)
}
