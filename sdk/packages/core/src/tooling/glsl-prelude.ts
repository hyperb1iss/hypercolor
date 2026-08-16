/**
 * Shared GLSL prelude injection (Spec 76 §1.6).
 *
 * `sdk/shared/glsl/color.glsl` is the single definition of the color helpers
 * fragment shaders reach for. Rather than every shader pasting its own copy,
 * the bundler injects the helpers a shader references and does not define
 * itself.
 *
 * Injection is per-symbol rather than a whole-file concat because GLSL rejects
 * a redefinition outright, and several effects legitimately override a helper
 * with their own tuned version. A shader that defines `rgb2hsv` keeps its own
 * and still gets `hsv2rgb` from here.
 */

import { COLOR_PRELUDE } from './color-prelude.gen'

interface PreludeFunction {
    name: string
    source: string
}

/** Split a GLSL source into its top-level function definitions. */
function parseFunctions(source: string): PreludeFunction[] {
    // Built per call: the scan drives lastIndex to skip over function bodies,
    // so a shared instance would carry one parse's cursor into the next.
    const head = /^([a-zA-Z_]\w*)[ \t]+([a-zA-Z_]\w*)[ \t]*\(/gm
    const functions: PreludeFunction[] = []
    let match = head.exec(source)
    while (match !== null) {
        const open = source.indexOf('{', match.index)
        if (open !== -1) {
            let depth = 0
            let end = -1
            for (let index = open; index < source.length; index++) {
                if (source[index] === '{') depth++
                else if (source[index] === '}') {
                    depth--
                    if (depth === 0) {
                        end = index + 1
                        break
                    }
                }
            }
            if (end !== -1) {
                functions.push({ name: match[2], source: source.slice(match.index, end) })
                head.lastIndex = end
            }
        }
        match = head.exec(source)
    }
    return functions
}

const PRELUDE_FUNCTIONS = parseFunctions(COLOR_PRELUDE)

function definedNames(source: string): Set<string> {
    return new Set(parseFunctions(source).map((fn) => fn.name))
}

function callsFunction(source: string, name: string): boolean {
    return new RegExp(`\\b${name}\\s*\\(`).test(source)
}

/**
 * The prelude helpers `shader` calls but does not define, in declaration
 * order, with helpers that other selected helpers depend on pulled in too.
 */
export function selectPreludeFunctions(shader: string): PreludeFunction[] {
    const defined = definedNames(shader)
    const selected = new Set<string>()

    const want = (name: string): void => {
        if (defined.has(name) || selected.has(name)) return
        const fn = PRELUDE_FUNCTIONS.find((candidate) => candidate.name === name)
        if (!fn) return
        selected.add(name)
        for (const dependency of PRELUDE_FUNCTIONS) {
            if (dependency.name !== name && callsFunction(fn.source, dependency.name)) want(dependency.name)
        }
    }

    for (const fn of PRELUDE_FUNCTIONS) {
        if (callsFunction(shader, fn.name)) want(fn.name)
    }

    return PRELUDE_FUNCTIONS.filter((fn) => selected.has(fn.name))
}

/**
 * Return `shader` with the prelude helpers it needs spliced in after the
 * `#version` directive, which GLSL requires to stay the first statement.
 */
export function injectColorPrelude(shader: string): string {
    const selected = selectPreludeFunctions(shader)
    if (selected.length === 0) return shader

    // GLSL ES 3.0 fragment shaders have no default float precision, and the
    // prelude lands above the shader's own declaration.
    const block = [
        '// ── hypercolor shared color prelude ──',
        'precision highp float;',
        ...selected.map((fn) => fn.source),
        '// ── end prelude ──',
    ].join('\n')

    const version = /^[ \t]*#version[^\n]*\n/.exec(shader)
    if (!version) return `${block}\n${shader}`
    const splice = version.index + version[0].length
    return `${shader.slice(0, splice)}${block}\n${shader.slice(splice)}`
}
