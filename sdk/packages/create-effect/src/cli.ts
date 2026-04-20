import { isTemplateKind, normalizeEffectId, normalizeWorkspaceName } from './naming'
import { promptWorkspaceOptions } from './prompts'
import { defaultSdkPackageSpec, resolveWorkspaceTarget, scaffoldWorkspace, workspaceNameFromTarget } from './scaffold'
import type { CliOutput, TemplateKind } from './types'

interface CliContext {
    cwd: string
    stdout: CliOutput
}

function optionValue(args: string[], name: string): string | undefined {
    const index = args.indexOf(name)
    if (index === -1) return undefined
    return args[index + 1]
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

function printHelp(stdout: CliOutput): void {
    stdout.log(`create-hypercolor-effect [name] [options]

Options:
  --template <type>       Starter template: canvas, shader, face, html
  --first <effect-name>   Name of the first effect (default: my-effect)
  --audio                 Include audio-reactive starter boilerplate
  --no-git                Skip git init
  --no-install            Skip bun install
  --sdk-spec <spec>       Override the generated @hypercolor/sdk dependency.
                          While the SDK is pre-release, point at a local
                          checkout: file:../hypercolor/sdk/packages/core
                          (HYPERCOLOR_SDK_PACKAGE_SPEC env var also works).
`)
}

export async function main(
    argv = process.argv.slice(2),
    context: CliContext = { cwd: process.cwd(), stdout: console },
): Promise<number> {
    if (argv.includes('--help') || argv.includes('help')) {
        printHelp(context.stdout)
        return 0
    }

    const [workspaceArg] = positionalArgs(argv)
    const templateArg = optionValue(argv, '--template')
    const templateCandidate = templateArg
    const firstArg = optionValue(argv, '--first')
    const sdkSpec = optionValue(argv, '--sdk-spec') ?? defaultSdkPackageSpec()
    const interactive = !workspaceArg || !templateArg
    const prompted = interactive
        ? await promptWorkspaceOptions({
              audio: argv.includes('--audio') ? true : undefined,
              effectId: firstArg ? normalizeEffectId(firstArg) : undefined,
              git: argv.includes('--no-git') ? false : undefined,
              install: argv.includes('--no-install') ? false : undefined,
              template: templateArg,
              workspaceName: workspaceArg ? normalizeWorkspaceName(workspaceArg) : undefined,
          })
        : undefined

    const workspaceName = prompted?.workspaceName ?? normalizeWorkspaceName(workspaceArg ?? '')
    const template: TemplateKind | undefined =
        prompted?.template ?? (templateCandidate && isTemplateKind(templateCandidate) ? templateCandidate : undefined)
    const firstEffectId = prompted?.effectId ?? normalizeEffectId(firstArg ?? 'my-effect')
    const audio = prompted?.audio ?? argv.includes('--audio')
    const git = prompted?.git ?? !argv.includes('--no-git')
    const install = prompted?.install ?? !argv.includes('--no-install')

    if (!workspaceName || !template) {
        context.stdout.error('Workspace name and template are required.')
        return 1
    }

    await scaffoldWorkspace({
        audio,
        firstEffectId,
        git,
        install,
        output: context.stdout,
        sdkPackageSpec: sdkSpec,
        targetDir: resolveWorkspaceTarget(context.cwd, workspaceName),
        template,
        workspaceName: workspaceNameFromTarget(workspaceName),
    })

    const nextCommand = template === 'html' ? 'bun run validate' : 'bun run build'
    context.stdout.log(`\nNext steps:\n  cd ${workspaceName}\n  ${nextCommand}`)

    return 0
}

if (import.meta.main) {
    const exitCode = await main()
    process.exit(exitCode)
}
