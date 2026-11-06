#!/usr/bin/env bun
/**
 * REST reference generator.
 *
 * Renders the daemon's OpenAPI document into two Zola pages:
 *   docs/content/api/rest.md           every operation, grouped by resource tag
 *   docs/content/api/rest-schemas.md   every component schema those operations use
 *
 * The document carries the contract: paths, parameters, bodies, statuses, and
 * the schemas behind them. Behavior the document cannot express (what a route
 * does with its input, examples, caveats) lives in the authored half,
 * docs/content/api/rest.notes.md, keyed by `METHOD /path`. A note whose key
 * names no current operation fails the run, so a removed route cannot leave
 * prose behind.
 *
 * Usage:
 *   bun scripts/gen-rest-reference.ts                export the document with Cargo, then write both pages
 *   bun scripts/gen-rest-reference.ts --check        fail if either page differs from what is on disk
 *   bun scripts/gen-rest-reference.ts --spec FILE    read an exported OpenAPI document instead of running Cargo
 *
 * The script is dependency-free, like gen-compat.ts.
 */

import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

// ─── Paths ──────────────────────────────────────────────────────────────

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const REPO_ROOT = resolve(SCRIPT_DIR, '..')
const API_CONTENT_DIR = join(REPO_ROOT, 'docs', 'content', 'api')
const REST_MD = join(API_CONTENT_DIR, 'rest.md')
const SCHEMAS_MD = join(API_CONTENT_DIR, 'rest-schemas.md')
const NOTES_MD = join(API_CONTENT_DIR, 'rest.notes.md')

const REST_PAGE = '@/api/rest.md'
const SCHEMAS_PAGE = '@/api/rest-schemas.md'
const ENVELOPE_PAGE = '@/api/rest-envelope-and-errors.md'
const OPENAPI_PAGE = '@/api/openapi.md'

const SCHEMA_REF_PREFIX = '#/components/schemas/'
const METHODS = ['get', 'post', 'put', 'patch', 'delete'] as const
const ENVELOPE_SCHEMAS = new Set(['ApiErrorBody', 'ApiErrorDetail', 'ResponseMeta'])
const SHARED_GROUP = 'shared'
const ENVELOPE_GROUP = 'envelope'
const UNREFERENCED_GROUP = 'unreferenced'
const MAX_FIELD_DEPTH = 3

// Every JSON Schema keyword the renderer understands. Anything else could
// change what a type means without changing the page, so it fails
// generation instead of being dropped.
const SCHEMA_KEYWORDS = new Set([
    '$ref',
    'additionalProperties',
    'allOf',
    'anyOf',
    'const',
    'default',
    'deprecated',
    'description',
    'enum',
    'example',
    'examples',
    'exclusiveMaximum',
    'exclusiveMinimum',
    'format',
    'items',
    'maxItems',
    'maxLength',
    'maximum',
    'minItems',
    'minLength',
    'minimum',
    'oneOf',
    'pattern',
    'properties',
    'propertyNames',
    'readOnly',
    'required',
    'title',
    'type',
    'uniqueItems',
    'writeOnly',
])
const SCHEMA_TYPES = new Set(['array', 'boolean', 'integer', 'null', 'number', 'object', 'string'])

// ─── Types ──────────────────────────────────────────────────────────────

// The OpenAPI document is JSON from a trusted generator; schemas are walked
// structurally, so a loose record type is the honest description.
export type Schema = Record<string, any>

interface Parameter {
    name: string
    in: string
    required?: boolean
    description?: string
    schema?: Schema
}

interface MediaContent {
    schema?: Schema
}

interface ResponseObject {
    description?: string
    content?: Record<string, MediaContent>
    headers?: Record<string, { description?: string; schema?: Schema }>
}

interface Operation {
    method: string
    path: string
    key: string
    id: string
    tag: string
    summary: string
    parameters: Parameter[]
    requestBody?: { required?: boolean; content?: Record<string, MediaContent> }
    responses: Record<string, ResponseObject>
}

interface Tag {
    name: string
    title: string
    description: string
}

interface Section {
    tag: string
    title: string
    id: string
    prefixes: string[]
    prose: string
}

export interface Notes {
    intro: string
    outro: string
    tagTitles: Map<string, string>
    tagProse: Map<string, string>
    sections: Section[]
    operations: Map<string, string>
}

export interface Document {
    openapi: string
    paths: Record<string, Record<string, Schema>>
    tags?: Array<{ name: string; description?: string }>
    components?: { schemas?: Record<string, Schema> }
}

interface Row {
    name: string
    type: string
    required: boolean
    description: string
}

// ─── Failure ────────────────────────────────────────────────────────────

export class GenerationError extends Error {}

function fail(message: string): never {
    throw new GenerationError(message)
}

// ─── Document loading ───────────────────────────────────────────────────

function cargoCacheCommand(): string[] {
    if (process.platform === 'win32') {
        return [
            'powershell.exe',
            '-NoLogo',
            '-NoProfile',
            '-ExecutionPolicy',
            'Bypass',
            '-File',
            join(REPO_ROOT, 'scripts', 'cargo-cache-build.ps1'),
        ]
    }
    return [join(REPO_ROOT, 'scripts', 'cargo-cache-build.sh')]
}

function exportDocument(): string {
    // Same invocation as python/scripts/generate_openapi_client.py, so a CI
    // job that already built the exporter for the Python client reuses it.
    const command = [
        ...cargoCacheCommand(),
        'cargo',
        'run',
        '-p',
        'hypercolor-daemon',
        '--bin',
        'hypercolor-openapi',
        '--no-default-features',
        '--quiet',
    ]
    const result = Bun.spawnSync(command, { cwd: REPO_ROOT, stdout: 'pipe', stderr: 'inherit' })
    if (result.exitCode !== 0) fail(`OpenAPI export failed with exit code ${result.exitCode}`)
    const stdout = result.stdout.toString()
    const start = stdout.indexOf('{')
    if (start === -1) fail('OpenAPI export stdout did not contain JSON')
    return stdout.slice(start)
}

function loadDocument(specPath: string | null): Document {
    const text = specPath ? readFileSync(specPath, 'utf8') : exportDocument()
    const document = JSON.parse(text) as Document
    if (!document.openapi?.startsWith('3.')) fail('input is not an OpenAPI 3 document')
    return document
}

// ─── Notes ──────────────────────────────────────────────────────────────

const NOTE_KEY = /^# (.+)$/
const FENCE = /^\s*(```|~~~)/
const OPERATION_KEY = /^(GET|POST|PUT|PATCH|DELETE) (\/\S*)$/

export function parseNotes(text: string): Notes {
    const notes: Notes = {
        intro: '',
        outro: '',
        tagTitles: new Map(),
        tagProse: new Map(),
        sections: [],
        operations: new Map(),
    }
    const seen = new Set<string>()
    let key: string | null = null
    let body: string[] = []
    let preamble: string[] = []
    let fence: string | null = null

    const flush = () => {
        if (key === null) return
        storeNote(notes, key, body.join('\n').trim())
        body = []
    }

    for (const line of text.split('\n')) {
        const fenceMatch = FENCE.exec(line)
        if (fenceMatch) {
            if (fence === null) fence = fenceMatch[1]
            else if (fence === fenceMatch[1]) fence = null
        }
        const keyMatch = fence === null ? NOTE_KEY.exec(line) : null
        if (keyMatch) {
            flush()
            key = keyMatch[1].trim()
            if (seen.has(key)) fail(`rest.notes.md: duplicate key \`${key}\``)
            seen.add(key)
            continue
        }
        if (key === null) preamble.push(line)
        else body.push(line)
    }
    if (fence !== null) fail('rest.notes.md: unterminated code fence')
    flush()

    const strayPreamble = preamble.join('\n').replace(/<!--[\s\S]*?-->/g, '').trim()
    if (strayPreamble) fail('rest.notes.md: text before the first `# ` key is never rendered; move it under a key')
    return notes
}

function storeNote(notes: Notes, key: string, prose: string) {
    if (key === '@intro') {
        notes.intro = prose
        return
    }
    if (key === '@outro') {
        notes.outro = prose
        return
    }
    const words = key.split(/\s+/)
    if (words[0] === '@tag') {
        const [, tag, ...title] = words
        if (!tag) fail(`rest.notes.md: \`${key}\` names no tag`)
        if (title.length) notes.tagTitles.set(tag, title.join(' '))
        if (prose) notes.tagProse.set(tag, prose)
        return
    }
    if (words[0] === '@section') {
        const [, tag, ...rest] = words
        const prefixes = rest.filter((word) => word.startsWith('/'))
        const title = rest.filter((word) => !word.startsWith('/')).join(' ')
        if (!tag || !title || prefixes.length === 0) {
            fail(`rest.notes.md: \`${key}\` must read \`@section <tag> <Title> </path/prefix>...\``)
        }
        notes.sections.push({ tag, title, id: `${tag}-${slugify(title)}`, prefixes, prose })
        return
    }
    if (OPERATION_KEY.test(key)) {
        notes.operations.set(key, prose)
        return
    }
    fail(`rest.notes.md: \`# ${key}\` is not \`METHOD /path\`, \`@tag\`, \`@section\`, \`@intro\`, or \`@outro\``)
}

// ─── Text helpers ───────────────────────────────────────────────────────

function slugify(text: string): string {
    return text
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/^-|-$/g, '')
}

/**
 * Order strings by UTF-16 code unit. `localeCompare` follows the runtime's
 * collation locale (Lithuanian sorts `y` with `i`), which would make the
 * committed pages differ from machine to machine.
 */
export function compareCodeUnits(a: string, b: string): number {
    return a < b ? -1 : a > b ? 1 : 0
}

function titleCase(word: string): string {
    return word.charAt(0).toUpperCase() + word.slice(1)
}

function code(text: string): string {
    const longest = Math.max(0, ...[...text.matchAll(/`+/g)].map((match) => match[0].length))
    const fence = '`'.repeat(longest + 1)
    const pad = text.startsWith('`') || text.endsWith('`') ? ' ' : ''
    return `${fence}${pad}${text}${pad}${fence}`
}

/**
 * Apply `transform` to the parts of `text` that sit outside code spans,
 * finding spans exactly as CommonMark does: a span opens on a backtick run
 * that no backslash escapes and closes only on the next run of the same
 * length. A run with no partner is literal text and gets transformed. Any
 * looser match would leave text unescaped that the renderer treats as
 * markup, so this must not be approximated.
 */
export function outsideCode(text: string, transform: (segment: string) => string): string {
    let out = ''
    let textStart = 0
    let index = 0
    while (index < text.length) {
        if (text[index] !== '`' || escapedAt(text, index)) {
            index += 1
            continue
        }
        const runEnd = backtickRunEnd(text, index)
        const close = closingRun(text, runEnd, runEnd - index)
        if (close === -1) {
            index = runEnd
            continue
        }
        out += transform(text.slice(textStart, index)) + text.slice(index, close)
        index = close
        textStart = close
    }
    return out + transform(text.slice(textStart))
}

/** Whether an odd number of backslashes precedes `index`. */
function escapedAt(text: string, index: number): boolean {
    let backslashes = 0
    for (let cursor = index - 1; cursor >= 0 && text[cursor] === '\\'; cursor -= 1) backslashes += 1
    return backslashes % 2 === 1
}

function backtickRunEnd(text: string, start: number): number {
    let end = start
    while (text[end] === '`') end += 1
    return end
}

/** End of the next backtick run of exactly `length` from `from`, or -1. */
function closingRun(text: string, from: number, length: number): number {
    let cursor = from
    while (cursor < text.length) {
        const open = text.indexOf('`', cursor)
        if (open === -1) return -1
        const end = backtickRunEnd(text, open)
        if (end - open === length) return end
        cursor = end
    }
    return -1
}

const SAFE_LINK_SCHEMES = new Set(['http', 'https', 'mailto'])

/** Fail on a markdown link whose target uses a scheme other than http(s) or mailto. */
function assertSafeLinks(segment: string): string {
    const targets = [
        ...[...segment.matchAll(/\]\(\s*<?([^\s)>]*)/g)].map((match) => match[1]),
        ...[...segment.matchAll(/^\s{0,3}\[[^\]]+\]:\s*<?(\S*?)>?(?:\s|$)/gm)].map((match) => match[1]),
    ]
    for (const target of targets) {
        const scheme = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(target)?.[1]?.toLowerCase()
        if (scheme && !SAFE_LINK_SCHEMES.has(scheme)) fail(`description links to an unsafe target: ${target}`)
    }
    return segment
}

function escapeHtml(segment: string): string {
    return segment.replace(/&(?![a-zA-Z#][a-zA-Z0-9]*;)/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

// ─── Rendering context ──────────────────────────────────────────────────

export class Reference {
    readonly schemas: Record<string, Schema>
    readonly operations: Operation[]
    readonly tags: Tag[]
    readonly notes: Notes
    readonly commonErrors: Array<[string, string]>
    readonly schemaGroups = new Map<string, string>()
    readonly schemaUsers = new Map<string, Array<{ operation: Operation; role: string }>>()

    constructor(document: Document, notes: Notes) {
        this.schemas = document.components?.schemas ?? {}
        this.notes = notes
        this.operations = collectOperations(document)
        this.validateSchemas()
        this.tags = collectTags(document, this.operations, notes)
        this.commonErrors = collectCommonErrors(this.operations)
        this.validateNotes()
        this.assignSchemaGroups()
    }

    schema(name: string): Schema {
        const schema = this.schemas[name]
        if (!schema) fail(`dangling schema reference \`${name}\``)
        return schema
    }

    /** Rustdoc-flavored description text, made safe for Zola markdown. */
    prose(text: string | undefined): string {
        if (!text) return ''
        if (/\{[{%]/.test(text)) fail(`description contains Zola template syntax: ${text.slice(0, 80)}`)
        // Rustdoc intra-doc links (`[`Type`]`) have no target in markdown.
        // Link the ones that name a schema; keep the rest as plain code.
        const linked = text.replace(/\[(`[^`\]]+`)\](?!\()/g, (_match, span: string) => {
            const name = span.slice(1, -1).split('::').pop() ?? ''
            return this.schemas[name] ? this.schemaLink(name) : span
        })
        return outsideCode(linked, (segment) => escapeHtml(assertSafeLinks(segment)))
    }

    /** Description text for a single table cell. */
    cell(text: string | undefined): string {
        return this.prose(text).replace(/\s+/g, ' ').trim().replace(/\|/g, '\\|')
    }

    schemaLink(name: string): string {
        this.schema(name)
        return `[${code(name)}](${SCHEMAS_PAGE}#${name})`
    }

    refName(ref: string): string {
        if (!ref.startsWith(SCHEMA_REF_PREFIX)) fail(`unsupported reference \`${ref}\``)
        return ref.slice(SCHEMA_REF_PREFIX.length)
    }

    // ─── Validation ─────────────────────────────────────────────────────

    private validateSchemas() {
        for (const [name, schema] of Object.entries(this.schemas)) {
            if (!/^[A-Za-z0-9_]+$/.test(name)) fail(`schema name \`${name}\` cannot serve as a page anchor`)
            this.validateSchema(schema, `components.schemas.${name}`)
        }
        for (const operation of this.operations) {
            for (const parameter of operation.parameters) {
                if (parameter.schema) this.validateSchema(parameter.schema, `${operation.key} parameter ${parameter.name}`)
            }
            for (const { schema, role } of operationSchemas(operation)) {
                if (role !== 'parameter') this.validateSchema(schema, `${operation.key} ${role}`)
            }
        }
    }

    private validateSchema(schema: unknown, path: string) {
        if (typeof schema === 'boolean') return
        if (!schema || typeof schema !== 'object' || Array.isArray(schema)) fail(`unsupported schema at ${path}`)
        const node = schema as Schema
        for (const key of Object.keys(node)) {
            if (!SCHEMA_KEYWORDS.has(key)) fail(`unsupported schema keyword \`${key}\` at ${path}`)
        }
        const types = Array.isArray(node.type) ? node.type : node.type === undefined ? [] : [node.type]
        for (const type of types) {
            if (!SCHEMA_TYPES.has(type)) fail(`unsupported schema type \`${type}\` at ${path}`)
        }
        if (typeof node.$ref === 'string') this.schema(this.refName(node.$ref))
        for (const [name, property] of Object.entries(node.properties ?? {})) {
            this.validateSchema(property, `${path}.properties.${name}`)
        }
        for (const keyword of ['items', 'additionalProperties', 'propertyNames']) {
            if (node[keyword] !== undefined) this.validateSchema(node[keyword], `${path}.${keyword}`)
        }
        for (const keyword of ['oneOf', 'anyOf', 'allOf']) {
            if (node[keyword] === undefined) continue
            if (!Array.isArray(node[keyword])) fail(`unsupported schema at ${path}.${keyword}: not a list`)
            node[keyword].forEach((member: unknown, index: number) =>
                this.validateSchema(member, `${path}.${keyword}[${index}]`),
            )
        }
        unionOf(node)
    }

    private validateNotes() {
        const keys = new Set(this.operations.map((operation) => operation.key))
        for (const key of this.notes.operations.keys()) {
            if (!keys.has(key)) fail(`rest.notes.md: \`# ${key}\` names no operation in the OpenAPI document`)
        }
        const tagNames = new Set(this.tags.map((tag) => tag.name))
        for (const tag of [...this.notes.tagTitles.keys(), ...this.notes.tagProse.keys()]) {
            if (!tagNames.has(tag)) fail(`rest.notes.md: \`@tag ${tag}\` names no tag in the OpenAPI document`)
        }
        for (const section of this.notes.sections) {
            if (!tagNames.has(section.tag)) {
                fail(`rest.notes.md: \`@section ${section.tag}\` names no tag in the OpenAPI document`)
            }
            for (const prefix of section.prefixes) {
                const matched = this.operations.some(
                    (operation) => operation.tag === section.tag && operation.path.startsWith(prefix),
                )
                if (!matched) {
                    fail(`rest.notes.md: section "${section.title}" prefix ${prefix} matches no ${section.tag} operation`)
                }
            }
        }
    }

    // ─── Schema grouping ────────────────────────────────────────────────

    private assignSchemaGroups() {
        const reach = new Map<string, Set<string>>()
        for (const operation of this.operations) {
            for (const { schema, role } of operationSchemas(operation)) {
                const direct = this.directRefs(schema)
                for (const name of direct) {
                    const users = this.schemaUsers.get(name) ?? []
                    users.push({ operation, role })
                    this.schemaUsers.set(name, users)
                }
                for (const name of this.reachable(direct)) {
                    const tags = reach.get(name) ?? new Set<string>()
                    tags.add(operation.tag)
                    reach.set(name, tags)
                }
            }
        }
        for (const name of Object.keys(this.schemas)) {
            const tags = reach.get(name)
            if (!tags) this.schemaGroups.set(name, ENVELOPE_SCHEMAS.has(name) ? ENVELOPE_GROUP : UNREFERENCED_GROUP)
            else this.schemaGroups.set(name, tags.size === 1 ? [...tags][0] : SHARED_GROUP)
        }
    }

    /** Schema names a node references directly, unwrapping the success envelope. */
    private directRefs(schema: Schema): string[] {
        const names = new Set<string>()
        const visit = (node: unknown) => {
            if (Array.isArray(node)) {
                for (const item of node) visit(item)
                return
            }
            if (!node || typeof node !== 'object') return
            const object = node as Schema
            if (typeof object.$ref === 'string') {
                const name = this.refName(object.$ref)
                if (name !== 'ResponseMeta') names.add(name)
                return
            }
            for (const value of Object.values(object)) visit(value)
        }
        visit(schema)
        return [...names]
    }

    private reachable(roots: string[]): Set<string> {
        const seen = new Set<string>()
        const stack = [...roots]
        while (stack.length) {
            const name = stack.pop() as string
            if (seen.has(name)) continue
            seen.add(name)
            stack.push(...this.directRefs(this.schema(name)))
        }
        return seen
    }

    // ─── Type expressions ───────────────────────────────────────────────

    /** A one-line, table-safe rendering of a schema's type. */
    typeExpr(schema: Schema | undefined): string {
        if (!schema) return 'any'
        if (typeof schema.$ref === 'string') return this.schemaLink(this.refName(schema.$ref))
        if (schema.const !== undefined) return code(JSON.stringify(schema.const))
        const union = unionOf(schema)
        if (Array.isArray(union)) {
            const nullable = union.some(isNullSchema)
            const variants = union.filter((variant: Schema) => !isNullSchema(variant))
            const values = enumValues({ oneOf: variants })
            const body = values
                ? unique(values.map((value) => (typeof value === 'string' ? 'string' : typeof value))).join(' or ')
                : unique(variants.map((variant: Schema) => this.typeExpr(variant))).join(' or ')
            return nullable ? `${body} or null` : body
        }
        if (Array.isArray(schema.allOf)) {
            return schema.allOf.map((part: Schema) => this.typeExpr(part)).join(' and ')
        }
        const types: string[] = Array.isArray(schema.type) ? schema.type : schema.type ? [schema.type] : []
        if (types.length === 0) return schema.properties ? 'object' : 'any'
        const nullable = types.includes('null')
        const concrete = types.filter((type) => type !== 'null')
        if (concrete.length === 0) return 'null'
        const body = concrete.map((type) => this.baseType(type, schema)).join(' or ')
        return nullable ? `${body} or null` : body
    }

    private baseType(type: string, schema: Schema): string {
        switch (type) {
            case 'array':
                return `array of ${this.typeExpr(schema.items)}`
            case 'object': {
                if (schema.properties) return 'object'
                const extra = schema.additionalProperties
                if (extra && typeof extra === 'object') return `map of ${this.typeExpr(extra)}`
                return extra === true ? 'map of any' : 'object'
            }
            case 'string':
                if (schema.format === 'binary') return 'binary'
                return schema.format ? `string (${schema.format})` : 'string'
            case 'integer':
            case 'number':
                return schema.format ? `${type} (${schema.format})` : type
            default:
                return type
        }
    }

    /** Constraint, enum, and default details appended to a field's description. */
    details(schema: Schema | undefined): string {
        if (!schema) return ''
        const target = unwrapNullable(schema)
        const parts: string[] = []
        const values = enumValues(target)
        if (values) parts.push(`One of ${values.map((value) => code(String(value))).join(', ')}.`)
        const bounds: string[] = []
        if (target.minimum !== undefined) bounds.push(`at least ${target.minimum}`)
        if (target.exclusiveMinimum !== undefined) bounds.push(`greater than ${target.exclusiveMinimum}`)
        if (target.maximum !== undefined) bounds.push(`at most ${target.maximum}`)
        if (target.exclusiveMaximum !== undefined) bounds.push(`less than ${target.exclusiveMaximum}`)
        if (target.minLength !== undefined) bounds.push(`at least ${target.minLength} characters`)
        if (target.maxLength !== undefined) bounds.push(`at most ${target.maxLength} characters`)
        if (target.minItems !== undefined) bounds.push(`at least ${target.minItems} items`)
        if (target.maxItems !== undefined) bounds.push(`at most ${target.maxItems} items`)
        // A bare unsigned bound restates the integer format; it is not news.
        const informative = bounds.filter((bound) => bound !== 'at least 0' || target.type === 'number')
        if (informative.length) parts.push(`${titleCase(informative.join(', '))}.`)
        if (target.pattern) parts.push(`Matches ${code(target.pattern)}.`)
        if (target.uniqueItems) parts.push('Items are unique.')
        if (schema.readOnly) parts.push('Read-only.')
        if (schema.writeOnly) parts.push('Write-only.')
        if (schema.deprecated) parts.push('Deprecated.')
        if (schema.default !== undefined) parts.push(`Default ${code(JSON.stringify(schema.default))}.`)
        return parts.join(' ')
    }

    // ─── Field tables ───────────────────────────────────────────────────

    fieldRows(schema: Schema, prefix = '', depth = 0, skip: Set<string> = new Set()): Row[] {
        const properties: Record<string, Schema> = schema.properties ?? {}
        const required = new Set<string>(schema.required ?? [])
        const rows: Row[] = []
        for (const [name, property] of Object.entries(properties)) {
            if (skip.has(name)) continue
            const description = [this.cell(property.description), this.cell(this.details(property))]
                .filter(Boolean)
                .join(' ')
            rows.push({
                name: `${prefix}${name}`,
                type: this.typeExpr(property),
                required: required.has(name),
                description,
            })
            if (depth + 1 >= MAX_FIELD_DEPTH) continue
            const inner = unwrapNullable(property)
            if (inner.properties) rows.push(...this.fieldRows(inner, `${prefix}${name}.`, depth + 1))
            const items = inner.items ? unwrapNullable(inner.items) : null
            if (items?.properties) rows.push(...this.fieldRows(items, `${prefix}${name}[].`, depth + 1))
        }
        return rows
    }

    fieldTable(rows: Row[]): string {
        // An all-empty Description column only costs width.
        const described = rows.some((row) => row.description)
        const lines = described
            ? ['| Field | Type | Required | Description |', '| --- | --- | --- | --- |']
            : ['| Field | Type | Required |', '| --- | --- | --- |']
        for (const row of rows) {
            const cells = [code(row.name), row.type, row.required ? 'yes' : 'no']
            if (described) cells.push(row.description)
            lines.push(`| ${cells.join(' | ')} |`)
        }
        return lines.join('\n')
    }

    // ─── Schema bodies ──────────────────────────────────────────────────

    /** Markdown describing a schema's shape: field tables, variants, or a type line. */
    schemaBody(schema: Schema): string {
        const blocks: string[] = []
        if (Array.isArray(schema.allOf)) {
            const bases = schema.allOf.filter((part: Schema) => part.$ref)
            const extra = schema.allOf.filter((part: Schema) => !part.$ref)
            for (const part of extra) {
                if (!part.properties || unionOf(part) || part.allOf) {
                    fail('unsupported allOf member: only schema references and objects with properties render')
                }
            }
            if (bases.length) {
                const links = bases.map((part: Schema) => this.typeExpr(part)).join(' and ')
                blocks.push(extra.length ? `Every field of ${links}, plus:` : `Every field of ${links}.`)
            }
            const rows = extra.flatMap((part: Schema) => this.fieldRows(part))
            if (rows.length) blocks.push(this.fieldTable(rows))
            return blocks.join('\n\n')
        }

        const union = unionOf(schema)
        if (Array.isArray(union)) {
            const concrete = union.filter((variant: Schema) => !isNullSchema(variant))
            if (concrete.length === 1 && concrete.length < union.length) {
                const inner = this.prose(concrete[0].description)
                return ['Either `null` or the value below.', inner, this.schemaBody(concrete[0])].filter(Boolean).join('\n\n')
            }
            if (!union.every((variant: Schema) => isNullSchema(variant) || enumValues(variant))) {
                return this.variantsBody(union)
            }
        }

        const values = enumValues(schema)
        if (values) return this.enumBody(schema, values)

        if (schema.properties) {
            const rows = this.fieldRows(schema)
            blocks.push(rows.length ? this.fieldTable(rows) : 'An object with no fields.')
            const extra = schema.additionalProperties
            if (extra === false) blocks.push('Unknown fields are rejected.')
            else if (extra === true || (extra && typeof extra === 'object')) {
                const type = extra === true ? 'any' : this.typeExpr(extra)
                blocks.push(type === 'any' ? 'Other keys may hold any JSON value.' : `Other keys map to ${type}.`)
            }
            return blocks.join('\n\n')
        }

        const items = schema.type === 'array' && schema.items ? unwrapNullable(schema.items) : null
        if (items?.properties) {
            const rows = this.fieldRows(items)
            return rows.length ? `An array of objects with these fields:\n\n${this.fieldTable(rows)}` : 'An array of objects.'
        }

        const type = this.typeExpr(schema)
        if (type === 'any') return 'Any JSON value.'
        return `Type: ${type}.`
    }

    private enumBody(schema: Schema, values: Array<string | number | boolean | null>): string {
        const union: Schema[] = unionOf(schema) ?? []
        const described = union.filter((variant) => variant.description && enumValues(variant))
        if (described.length === 0) {
            return `One of ${values.map((value) => code(String(value))).join(', ')}.`
        }
        const lines = ['| Value | Description |', '| --- | --- |']
        for (const variant of union) {
            const variantValues = enumValues(variant)
            if (!variantValues) continue
            for (const value of variantValues) {
                lines.push(`| ${code(String(value))} | ${this.cell(variant.description)} |`)
            }
        }
        return lines.join('\n')
    }

    private variantsBody(union: Schema[]): string {
        const variants = union.filter((variant) => !isNullSchema(variant))
        const objects = variants.filter((variant) => variant.properties)
        const discriminator = findDiscriminator(objects)
        const skip = new Set(discriminator ? [discriminator] : [])
        const blocks: string[] = []
        blocks.push(
            discriminator
                ? `One of these variants, told apart by ${code(discriminator)}:`
                : 'One of these variants:',
        )
        if (union.some(isNullSchema)) blocks.push('The value may also be `null`.')

        const rowsOf = (variant: Schema) => (variant.properties ? this.fieldRows(variant, '', 0, skip) : [])
        const variantNote = (variant: Schema) => this.cell(variant.description)
        // Small variants read best as one table, a row per variant. A variant
        // with several fields, or fields that carry their own descriptions,
        // gets a block with a full field table instead.
        const compact = variants.every((variant) => {
            if (!variant.properties) return describedByType(variant)
            const rows = rowsOf(variant)
            return rows.length <= 3 && rows.every((row) => !row.description || row.description === variantNote(variant))
        })
        if (compact) {
            const described = variants.some((variant) => variantNote(variant))
            const lines = [
                `| ${discriminator ? code(discriminator) : 'Variant'} | Fields |${described ? ' Description |' : ''}`,
                `| --- | --- |${described ? ' --- |' : ''}`,
            ]
            variants.forEach((variant, index) => {
                const tagged = discriminator && variant.properties?.[discriminator]
                const label = tagged
                    ? code(JSON.stringify(enumValues(variant.properties[discriminator])?.[0]))
                    : this.variantLabel(variant, null, index)
                const fields = variant.properties
                    ? rowsOf(variant)
                          .map((row) => `${code(row.name)}: ${row.type}${row.required ? '' : ' (optional)'}`)
                          .join('<br>') || 'none'
                    : ''
                lines.push(`| ${label} | ${fields} |${described ? ` ${variantNote(variant)} |` : ''}`)
            })
            blocks.push(lines.join('\n'))
            return blocks.join('\n\n')
        }

        variants.forEach((variant, index) => {
            const label = this.variantLabel(variant, discriminator, index)
            const description = variantNote(variant)
            blocks.push(description ? `**${label}**: ${description}` : `**${label}**`)
            const rows = rowsOf(variant)
            if (rows.length) blocks.push(this.fieldTable(rows))
            else if (!variant.properties && !describedByType(variant)) blocks.push(this.schemaBody(variant))
        })
        return blocks.join('\n\n')
    }

    private variantLabel(variant: Schema, discriminator: string | null, index: number): string {
        const values = enumValues(variant)
        if (values) return values.map((value) => code(JSON.stringify(value))).join(', ')
        if (!variant.properties) return this.typeExpr(variant)
        if (discriminator && variant.properties[discriminator]) {
            const tag = enumValues(variant.properties[discriminator])?.[0]
            return code(`${discriminator}: ${JSON.stringify(tag)}`)
        }
        const keys = Object.keys(variant.properties)
        if (keys.length === 1 && variant.required?.includes(keys[0])) return code(`{ "${keys[0]}": … }`)
        return `Variant ${index + 1}`
    }
}

// ─── Schema predicates ──────────────────────────────────────────────────

/** A schema's `oneOf` or `anyOf` members, which render the same way. */
function unionOf(schema: Schema): Schema[] | null {
    if (schema.oneOf && schema.anyOf) fail('unsupported schema: both `oneOf` and `anyOf` on one node')
    const union = schema.oneOf ?? schema.anyOf
    return Array.isArray(union) ? union : null
}

/** Whether `typeExpr` says everything about a schema, so it needs no field table. */
function describedByType(schema: Schema): boolean {
    if (schema.properties || schema.allOf) return false
    if (schema.items && !describedByType(schema.items)) return false
    return (unionOf(schema) ?? []).every(describedByType)
}

function isNullSchema(schema: Schema): boolean {
    return schema.type === 'null' && Object.keys(schema).every((key) => key === 'type' || key === 'description')
}

function unwrapNullable(schema: Schema): Schema {
    const union = unionOf(schema)
    if (Array.isArray(union)) {
        const concrete = union.filter((variant: Schema) => !isNullSchema(variant))
        if (concrete.length === 1) return concrete[0]
    }
    return schema
}

/** Literal values when a schema is a closed set of scalars, else null. */
function enumValues(schema: Schema | undefined): Array<string | number | boolean | null> | null {
    if (!schema) return null
    if (Array.isArray(schema.enum)) return schema.enum.filter((value: unknown) => value !== null)
    const union = unionOf(schema)
    if (!Array.isArray(union) || union.length === 0) return null
    const values: Array<string | number | boolean | null> = []
    for (const variant of union as Schema[]) {
        if (isNullSchema(variant)) continue
        if (Array.isArray(variant.enum)) values.push(...variant.enum)
        else if (variant.const !== undefined) values.push(variant.const)
        else return null
    }
    return values.length ? values : null
}

function findDiscriminator(objects: Schema[]): string | null {
    if (objects.length === 0) return null
    const candidates = Object.keys(objects[0].properties ?? {})
    for (const name of candidates) {
        const tagged = objects.every((variant) => {
            const values = enumValues(variant.properties?.[name])
            return values?.length === 1 && variant.required?.includes(name)
        })
        if (tagged) return name
    }
    return null
}

function unique<T>(items: T[]): T[] {
    return [...new Set(items)]
}

// ─── Operations ─────────────────────────────────────────────────────────

function collectOperations(document: Document): Operation[] {
    const operations: Operation[] = []
    const ids = new Set<string>()
    for (const [path, item] of Object.entries(document.paths ?? {})) {
        for (const method of METHODS) {
            const raw = item[method]
            if (!raw) continue
            const key = `${method.toUpperCase()} ${path}`
            const tags: string[] = raw.tags ?? []
            if (tags.length !== 1) fail(`${key} must carry exactly one tag, found ${tags.length}`)
            if (!raw.operationId) fail(`${key} has no operationId`)
            if (ids.has(raw.operationId)) fail(`duplicate operationId \`${raw.operationId}\``)
            ids.add(raw.operationId)
            if (!raw.summary) fail(`${key} has no summary`)
            // The summary becomes a component argument, where Zola would read
            // template syntax as markup.
            if (/\{[{%]/.test(raw.summary)) fail(`${key} summary contains Zola template syntax`)
            operations.push({
                method: method.toUpperCase(),
                path,
                key,
                id: raw.operationId,
                tag: tags[0],
                summary: raw.summary,
                parameters: raw.parameters ?? [],
                requestBody: raw.requestBody,
                responses: raw.responses ?? {},
            })
        }
    }
    // Sorting by path keeps collections ahead of their `{id}` items (`{`
    // sorts after letters) and keeps router registration order out of the
    // diff. Paths outside /api/v1 (`/health`) lead their tag.
    const rank = (operation: Operation) => (operation.path.startsWith('/api/v1/') ? 1 : 0)
    return operations.sort(
        (a, b) =>
            rank(a) - rank(b) ||
            compareCodeUnits(a.path, b.path) ||
            METHODS.indexOf(a.method.toLowerCase() as (typeof METHODS)[number]) -
                METHODS.indexOf(b.method.toLowerCase() as (typeof METHODS)[number]),
    )
}

function collectTags(document: Document, operations: Operation[], notes: Notes): Tag[] {
    const used = new Set(operations.map((operation) => operation.tag))
    const declared = document.tags ?? []
    // A tag's title, description, and position come from its declaration in
    // ApiDoc (crates/hypercolor-daemon/src/api/openapi.rs).
    for (const name of [...used].sort(compareCodeUnits)) {
        if (!declared.some((tag) => tag.name === name)) {
            fail(`operations use tag \`${name}\`, which ApiDoc does not declare in api/openapi.rs`)
        }
    }
    return declared
        .filter((tag) => used.has(tag.name))
        .map((tag) => ({
            name: tag.name,
            title: notes.tagTitles.get(tag.name) ?? titleCase(tag.name),
            description: tag.description ?? '',
        }))
}

function isErrorStatus(status: string): boolean {
    return /^[45]/.test(status)
}

/** Error responses every operation declares identically, in status order. */
function collectCommonErrors(operations: Operation[]): Array<[string, string]> {
    // Headers stay out of the signature: idempotent writes add
    // `Idempotency-Replayed` to every response, errors included.
    const signature = (response: ResponseObject) => JSON.stringify([response.description, response.content])
    const first = operations[0]
    if (!first) return []
    const common: Array<[string, string]> = []
    for (const [status, response] of Object.entries(first.responses)) {
        if (!isErrorStatus(status)) continue
        const shared = operations.every(
            (operation) => operation.responses[status] && signature(operation.responses[status]) === signature(response),
        )
        if (shared) common.push([status, response.description ?? ''])
    }
    return common.sort(([a], [b]) => compareCodeUnits(a, b))
}

function operationSchemas(operation: Operation): Array<{ schema: Schema; role: string }> {
    const found: Array<{ schema: Schema; role: string }> = []
    for (const content of Object.values(operation.requestBody?.content ?? {})) {
        if (content.schema) found.push({ schema: content.schema, role: 'request' })
    }
    for (const [status, response] of Object.entries(operation.responses)) {
        if (isErrorStatus(status)) continue
        for (const content of Object.values(response.content ?? {})) {
            if (content.schema) found.push({ schema: content.schema, role: 'response' })
        }
    }
    for (const parameter of operation.parameters) {
        if (parameter.schema) found.push({ schema: parameter.schema, role: 'parameter' })
    }
    return found
}

/** The `data` schema of an enveloped response, or null for a raw body. */
function envelopeData(schema: Schema): Schema | null {
    const properties = schema.properties
    if (!properties?.data || properties.meta?.$ref !== `${SCHEMA_REF_PREFIX}ResponseMeta`) return null
    return properties.data
}

// ─── Component arguments ────────────────────────────────────────────────

function componentArg(name: string, value: string): string {
    for (const quote of ['"', "'", '`']) {
        if (!value.includes(quote)) return `${name}=${quote}${value}${quote}`
    }
    fail(`cannot quote component argument ${name}: ${value}`)
}

// ─── rest.md ────────────────────────────────────────────────────────────

const GENERATED_BANNER = `<!--
Generated by scripts/gen-rest-reference.ts from the daemon's OpenAPI
document and docs/content/api/rest.notes.md. Do not edit this file by
hand: change the route's OperationDoc or the notes, then run
\`just rest-reference\`.
-->`

export function renderRest(ref: Reference): string {
    const operationCount = ref.operations.length
    const pathCount = new Set(ref.operations.map((operation) => operation.path)).size
    const out: string[] = [
        '+++',
        'title = "REST API reference"',
        'description = "Every operation the Hypercolor daemon serves over HTTP, generated from its OpenAPI document: parameters, bodies, statuses, and schemas."',
        'weight = 10',
        'template = "page.html"',
        '+++',
        '',
        GENERATED_BANNER,
        '',
    ]
    if (ref.notes.intro) out.push(ref.notes.intro, '')
    out.push(
        `This reference covers ${operationCount} operations on ${pathCount} paths. It is generated from the daemon's [OpenAPI document](${OPENAPI_PAGE}), so it lists exactly what the router serves. Request and response types link to their definitions in [REST schemas](${SCHEMAS_PAGE}).`,
        '',
    )

    if (ref.commonErrors.length) {
        out.push('## Shared error responses {#shared-errors}', '')
        out.push(
            `The document declares these statuses on every operation, each answering with the [error envelope](${ENVELOPE_PAGE}) (${ref.schemaLink('ApiErrorBody')}). The cards below list only the statuses an operation adds.`,
            '',
            '| Status | Meaning |',
            '| --- | --- |',
        )
        for (const [status, description] of ref.commonErrors) out.push(`| \`${status}\` | ${ref.cell(description)} |`)
        out.push('')
    }

    for (const tag of ref.tags) {
        const operations = ref.operations.filter((operation) => operation.tag === tag.name)
        const sections = ref.notes.sections.filter((section) => section.tag === tag.name)
        const sectionOf = (operation: Operation) => {
            let best: Section | null = null
            let bestLength = -1
            for (const section of sections) {
                for (const prefix of section.prefixes) {
                    if (operation.path.startsWith(prefix) && prefix.length > bestLength) {
                        best = section
                        bestLength = prefix.length
                    }
                }
            }
            return best
        }

        out.push(`## ${tag.title} {#${tag.name}}`, '')
        if (tag.description) out.push(`${ref.prose(tag.description)}.`.replace(/\.\.$/, '.'), '')
        const tagProse = ref.notes.tagProse.get(tag.name)
        if (tagProse) out.push(tagProse, '')
        for (const operation of operations.filter((operation) => sectionOf(operation) === null)) {
            out.push(renderCard(ref, operation), '')
        }
        for (const section of sections) {
            const members = operations.filter((operation) => sectionOf(operation) === section)
            if (members.length === 0) {
                fail(`rest.notes.md: section "${section.title}" holds no operation; a longer prefix claims them all`)
            }
            out.push(`### ${section.title} {#${section.id}}`, '')
            if (section.prose) out.push(section.prose, '')
            for (const operation of members) out.push(renderCard(ref, operation), '')
        }
    }

    if (ref.notes.outro) out.push('---', '', ref.notes.outro, '')
    return `${out.join('\n').replace(/\n{3,}/g, '\n\n').trimEnd()}\n`
}

function renderCard(ref: Reference, operation: Operation): string {
    const args = [
        componentArg('method', operation.method),
        componentArg('path', operation.path),
        componentArg('id', operation.id),
        componentArg('summary', operation.summary),
    ].join(' ')
    const blocks: string[] = []
    const notes = ref.notes.operations.get(operation.key)
    if (notes) blocks.push(notes)

    const parameters = renderParameters(ref, operation)
    if (parameters) blocks.push(parameters)
    const request = renderRequest(ref, operation)
    if (request) blocks.push(request)
    blocks.push(renderResponses(ref, operation))

    return `{% <api_endpoint ${args}> %}\n${blocks.join('\n\n')}\n{% </api_endpoint> %}`
}

function renderParameters(ref: Reference, operation: Operation): string {
    const parameters = operation.parameters
    if (parameters.length === 0) return ''
    const plainPath = (parameter: Parameter) =>
        parameter.in === 'path' && !parameter.description && ref.typeExpr(parameter.schema) === 'string'
    if (parameters.every(plainPath)) {
        const names = parameters.map((parameter) => code(parameter.name)).join(', ')
        return `**Path parameters**: ${names} (${parameters.length === 1 ? 'string' : 'strings'}).`
    }
    const lines = ['**Parameters**', '', '| Name | In | Type | Required | Description |', '| --- | --- | --- | --- | --- |']
    for (const parameter of parameters) {
        const description = [ref.cell(parameter.description), ref.cell(ref.details(parameter.schema))]
            .filter(Boolean)
            .join(' ')
        lines.push(
            `| ${code(parameter.name)} | ${parameter.in} | ${ref.typeExpr(parameter.schema)} | ${parameter.required ? 'yes' : 'no'} | ${description} |`,
        )
    }
    return lines.join('\n')
}

function renderRequest(ref: Reference, operation: Operation): string {
    const body = operation.requestBody
    if (!body?.content) return ''
    const blocks: string[] = []
    for (const [mediaType, content] of Object.entries(body.content)) {
        const schema = content.schema ?? {}
        const requirement = body.required ? 'required' : 'optional'
        const type = ref.typeExpr(schema)
        const shape = type === 'binary' ? 'the raw bytes' : type
        blocks.push(`**Request body** (${code(mediaType)}, ${requirement}): ${shape}.`)
        if (typeof schema.$ref === 'string') {
            const resolved = ref.schema(ref.refName(schema.$ref))
            if (resolved.properties || resolved.allOf) blocks.push(ref.schemaBody(resolved))
        }
    }
    return blocks.join('\n\n')
}

function renderResponses(ref: Reference, operation: Operation): string {
    const common = new Set(ref.commonErrors.map(([status]) => status))
    const lines = ['**Responses**', '', '| Status | Body | Meaning |', '| --- | --- | --- |']
    const headers = new Map<string, string>()
    let fields: { title: string; schema: Schema } | null = null

    const statuses = Object.keys(operation.responses).sort(compareCodeUnits)
    for (const status of statuses) {
        if (common.has(status)) continue
        const response = operation.responses[status]
        const media = Object.entries(response.content ?? {})
        let body = 'none'
        if (media.length) {
            const [, first] = media[0]
            const schema = first.schema ?? {}
            const data = envelopeData(schema)
            if (data) {
                body = `${code('data')}: ${ref.typeExpr(data)}`
                if (!fields && !isErrorStatus(status)) fields = responseFields(ref, data)
            } else if (media.every(([, content]) => ref.typeExpr(content.schema) === 'binary')) {
                body = `${media.map(([type]) => code(type)).join(' or ')} bytes`
            } else {
                body = `${ref.typeExpr(schema)}, no envelope`
                if (!fields && !isErrorStatus(status)) fields = responseFields(ref, schema)
            }
        }
        const generic = `${operation.summary} response`
        const meaning = response.description === generic ? defaultMeaning(status) : ref.cell(response.description)
        lines.push(`| \`${status}\` | ${body} | ${meaning} |`)
        for (const [name, header] of Object.entries(response.headers ?? {})) {
            headers.set(name, ref.cell(header.description))
        }
    }

    const blocks = [lines.join('\n')]
    if (headers.size) {
        const headerLines = ['| Response header | Meaning |', '| --- | --- |']
        for (const [name, description] of headers) headerLines.push(`| ${code(name)} | ${description} |`)
        blocks.push(headerLines.join('\n'))
    }
    if (fields) {
        const body = ref.schemaBody(fields.schema)
        blocks.push(`<details>\n<summary>${fields.title}</summary>\n\n${body}\n\n</details>`)
    }
    return blocks.join('\n\n')
}

function defaultMeaning(status: string): string {
    switch (status) {
        case '101':
            return 'Switching protocols'
        case '200':
            return 'Success'
        case '201':
            return 'Created'
        case '202':
            return 'Accepted; the work completes asynchronously'
        case '204':
            return 'Success, no body'
        case '503':
            return 'Service unavailable'
        default:
            return ''
    }
}

/** The field table worth showing for a success body, or null when it is not an object. */
function responseFields(ref: Reference, schema: Schema): { title: string; schema: Schema } | null {
    if (typeof schema.$ref !== 'string') return null
    const name = ref.refName(schema.$ref)
    const resolved = ref.schema(name)
    const hasFields = (candidate: Schema) =>
        Boolean(candidate.properties || candidate.allOf || candidate.items?.properties)
    // A list body's own fields are `items`, `total`, and `page`, and a bare
    // array has none; the item type is the part a reader came for.
    const listItems = resolved.properties?.total ? resolved.properties?.items : null
    const items = listItems ?? (resolved.type === 'array' ? resolved : null)
    if (items) {
        if (typeof items.items?.$ref === 'string') {
            const itemName = ref.refName(items.items.$ref)
            const itemSchema = ref.schema(itemName)
            return hasFields(itemSchema) ? { title: `Item fields: <code>${itemName}</code>`, schema: itemSchema } : null
        }
        return items.items?.properties ? { title: 'Item fields', schema: items.items } : null
    }
    return hasFields(resolved) ? { title: `Fields: <code>${name}</code>`, schema: resolved } : null
}

// ─── rest-schemas.md ────────────────────────────────────────────────────

export function renderSchemas(ref: Reference): string {
    const out: string[] = [
        '+++',
        'title = "REST schemas"',
        'description = "Every request and response type in the Hypercolor REST API, generated from the daemon\'s OpenAPI document."',
        'weight = 15',
        'template = "page.html"',
        '',
        '[extra]',
        'toc_depth = 2',
        '+++',
        '',
        GENERATED_BANNER,
        '',
        `Every type the [REST reference](${REST_PAGE}) links to, generated from the daemon's [OpenAPI document](${OPENAPI_PAGE}). Types are grouped by the resource whose operations use them. Types used by several resources sit under Shared, and the response envelope's own types sit under Envelope.`,
        '',
    ]

    const groups = [
        ...ref.tags.map((tag) => ({ id: tag.name, title: tag.title })),
        { id: SHARED_GROUP, title: 'Shared' },
        { id: ENVELOPE_GROUP, title: 'Envelope' },
        { id: UNREFERENCED_GROUP, title: 'Unreferenced' },
    ]
    for (const group of groups) {
        const names = Object.keys(ref.schemas)
            .filter((name) => ref.schemaGroups.get(name) === group.id)
            .sort(compareCodeUnits)
        if (names.length === 0) continue
        out.push(`## ${group.title} {#group-${group.id}}`, '')
        if (group.id === UNREFERENCED_GROUP) {
            out.push('The document declares these types, but no operation references them.', '')
        }
        for (const name of names) out.push(renderSchema(ref, name), '')
    }
    return `${out.join('\n').replace(/\n{3,}/g, '\n\n').trimEnd()}\n`
}

function renderSchema(ref: Reference, name: string): string {
    const schema = ref.schema(name)
    const blocks = [`### ${name} {#${name}}`]
    const description = ref.prose(schema.description)
    if (description) blocks.push(description)
    blocks.push(ref.schemaBody(schema))
    const users = ref.schemaUsers.get(name) ?? []
    if (users.length) {
        const seen = new Set<string>()
        const links: string[] = []
        for (const { operation, role } of users) {
            const label = `${operation.key} ${role}`
            if (seen.has(label)) continue
            seen.add(label)
            links.push(`[${code(operation.key)}](${REST_PAGE}#${operation.id}) ${role}`)
        }
        blocks.push(`Used by ${links.join(', ')}.`)
    }
    return blocks.join('\n\n')
}

// ─── Output ─────────────────────────────────────────────────────────────

function writeOrCheck(path: string, contents: string, check: boolean): boolean {
    const rel = relative(REPO_ROOT, path)
    const previous = existsSync(path) ? readFileSync(path, 'utf8') : null
    if (previous === contents) {
        console.log(`  ✓ ${rel} is current`)
        return false
    }
    if (check) {
        console.error(`  ✗ ${rel} is out of date`)
        return true
    }
    writeFileSync(path, contents)
    console.log(`  ✓ ${rel} written`)
    return true
}

function parseArgs(argv: string[]): { check: boolean; spec: string | null } {
    let check = false
    let spec: string | null = null
    for (let index = 0; index < argv.length; index += 1) {
        const arg = argv[index]
        if (arg === '--check') check = true
        else if (arg === '--spec') {
            spec = argv[index + 1] ?? fail('--spec needs a file path')
            index += 1
        } else fail(`unknown argument ${arg}; usage: gen-rest-reference.ts [--check] [--spec FILE]`)
    }
    return { check, spec }
}

function main() {
    const { check, spec } = parseArgs(process.argv.slice(2))
    console.log(check ? '→ Checking the REST reference is current…' : '→ Generating the REST reference…')
    const document = loadDocument(spec)
    const notes = parseNotes(readFileSync(NOTES_MD, 'utf8'))
    const ref = new Reference(document, notes)

    const outputs: Array<[string, string]> = [
        [REST_MD, renderRest(ref)],
        [SCHEMAS_MD, renderSchemas(ref)],
    ]
    let changed = false
    for (const [path, contents] of outputs) {
        if (writeOrCheck(path, contents, check)) changed = true
    }

    const schemaCount = Object.keys(ref.schemas).length
    console.log(`\n  ${ref.operations.length} operations · ${ref.tags.length} resources · ${schemaCount} schemas`)
    if (check && changed) {
        console.error('\n✗ The REST reference is out of date. Run `just rest-reference` to regenerate.')
        process.exit(1)
    }
    console.log(check ? '\n✓ The REST reference is current.' : '\n✓ The REST reference is generated.')
}

// Importing the module (scripts/tests/gen-rest-reference.test.ts) must not
// run a generation.
if (import.meta.main) {
    try {
        main()
    } catch (error) {
        if (error instanceof GenerationError) {
            console.error(`\n✗ ${error.message}`)
            process.exit(1)
        }
        throw error
    }
}
