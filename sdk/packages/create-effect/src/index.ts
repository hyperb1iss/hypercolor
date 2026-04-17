export { main } from './cli'
export { displayNameFromEffectId, normalizeEffectId, normalizeWorkspaceName } from './naming'
export { promptAddEffectOptions, promptWorkspaceOptions } from './prompts'
export {
    addEffect,
    defaultSdkPackageSpec,
    resolveEditor,
    resolveWorkspaceTarget,
    scaffoldWorkspace,
    workspaceNameFromTarget,
} from './scaffold'
export type {
    AddEffectOptions,
    AddEffectResult,
    CliOutput,
    PromptedAddEffectOptions,
    PromptedScaffoldOptions,
    ScaffoldWorkspaceOptions,
    ScaffoldWorkspaceResult,
    TemplateKind,
} from './types'
