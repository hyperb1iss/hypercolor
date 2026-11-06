import { describe, expect, test } from 'bun:test'

import {
    type Document,
    GenerationError,
    parseNotes,
    Reference,
    renderRest,
    renderSchemas,
    type Schema,
} from '../gen-rest-reference.ts'

const XSS = '<img src=x onerror=alert(1)>'

/** A one-operation document whose success body references every given schema. */
function documentWith(schemas: Record<string, Schema>): Document {
    const properties = Object.fromEntries(
        Object.keys(schemas).map((name) => [name, { $ref: `#/components/schemas/${name}` }]),
    )
    return {
        openapi: '3.1.0',
        tags: [{ name: 'things', description: 'Things' }],
        paths: {
            '/api/v1/things': {
                get: {
                    tags: ['things'],
                    operationId: 'list_things',
                    summary: 'List things',
                    responses: {
                        '200': {
                            description: 'List things response',
                            content: {
                                'application/json': {
                                    schema: {
                                        type: 'object',
                                        required: ['data', 'meta'],
                                        properties: {
                                            data: { $ref: '#/components/schemas/Things' },
                                            meta: { $ref: '#/components/schemas/ResponseMeta' },
                                        },
                                    },
                                },
                            },
                        },
                    },
                },
            },
        },
        components: {
            schemas: {
                ...schemas,
                Things: { type: 'object', properties },
                ResponseMeta: { type: 'object', properties: { request_id: { type: 'string' } } },
                ApiErrorBody: { type: 'object', properties: { error: { type: 'string' } } },
            },
        },
    }
}

function reference(schemas: Record<string, Schema>): Reference {
    return new Reference(documentWith(schemas), parseNotes(''))
}

/** The rendered markdown for one schema on the schemas page. */
function schemaSection(page: string, name: string): string {
    const start = page.indexOf(`### ${name} {#${name}}`)
    expect(start).toBeGreaterThanOrEqual(0)
    const next = page.indexOf('\n### ', start + 1)
    return page.slice(start, next === -1 ? undefined : next)
}

describe('ordering', () => {
    test('schema order ignores the runtime collation locale', () => {
        const original = String.prototype.localeCompare
        // Lithuanian collation sorts `y` with `i`, which flips these two names.
        const lithuanian = new Intl.Collator('lt')
        String.prototype.localeCompare = function (this: string, other: string) {
            return lithuanian.compare(String(this), other)
        }
        try {
            expect('ApplyControlChangesResponse'.localeCompare('AppliedControlChange')).toBeLessThan(0)
            const page = renderSchemas(
                reference({
                    ApplyControlChangesResponse: { type: 'object', properties: { ok: { type: 'boolean' } } },
                    AppliedControlChange: { type: 'object', properties: { key: { type: 'string' } } },
                }),
            )
            expect(page.indexOf('### AppliedControlChange ')).toBeLessThan(
                page.indexOf('### ApplyControlChangesResponse '),
            )
        } finally {
            String.prototype.localeCompare = original
        }
    })

    test('rendering is identical under a different collation locale', () => {
        const schemas = {
            Zeta: { type: 'object', properties: { b: { type: 'string' }, a: { type: 'string' } } },
            alpha: { type: 'string', enum: ['y', 'i', 'Z'] },
            Ypsilon: { type: 'integer' },
        }
        const baseline = renderSchemas(reference(schemas)) + renderRest(reference(schemas))
        const original = String.prototype.localeCompare
        const lithuanian = new Intl.Collator('lt')
        String.prototype.localeCompare = function (this: string, other: string) {
            return lithuanian.compare(String(this), other)
        }
        try {
            expect(renderSchemas(reference(schemas)) + renderRest(reference(schemas))).toBe(baseline)
        } finally {
            String.prototype.localeCompare = original
        }
    })
})

describe('description escaping', () => {
    const prose = (text: string) => reference({}).prose(text)

    test('raw HTML in prose is escaped', () => {
        const out = prose(`A ${XSS} tag`)
        expect(out).not.toContain('<img')
        expect(out).toContain('&lt;img src=x onerror=alert(1)&gt;')
    })

    test('a backtick run inside a longer run never closes a code span', () => {
        // CommonMark closes a code span only with a run of exactly the
        // opening length. Here no run has a partner, so every backtick is
        // literal and the tag is text.
        const out = prose(`\`\`x\`\`\`${XSS}\``)
        expect(out).not.toContain('<img')
        expect(out).toContain('&lt;img')
    })

    test('a single backtick is not closed by the start of a longer run', () => {
        const out = prose(`\`${XSS}\`\`\``)
        expect(out).not.toContain('<img')
    })

    test('a backslash-escaped backtick does not open a code span', () => {
        const out = prose(`\\\`${XSS}\``)
        expect(out).not.toContain('<img')
    })

    test('real code spans keep their text verbatim', () => {
        expect(prose('the fence `"<run_epoch>:<revision>"` here')).toContain('`"<run_epoch>:<revision>"`')
        expect(prose('``a ` <b>``')).toBe('``a ` <b>``')
    })

    test('template syntax in an operation summary fails generation', () => {
        const document = documentWith({})
        document.paths['/api/v1/things'].get.summary = 'List {{ things }}'
        expect(() => new Reference(document, parseNotes(''))).toThrow(/template syntax/)
    })

    test('an unsafe link target fails generation', () => {
        expect(() => prose('[click](javascript:alert(1))')).toThrow(GenerationError)
        expect(() => prose('[docs](https://example.com) and [x](#anchor)')).not.toThrow()
    })

    test('the schemas page carries no raw HTML from descriptions', () => {
        const page = renderSchemas(
            reference({
                Hostile: {
                    type: 'object',
                    description: `\`\`x\`\`\`${XSS}\``,
                    properties: { field: { type: 'string', description: `\`${XSS}\`\`\`` } },
                },
            }),
        )
        expect(page).not.toContain('<img')
    })
})

describe('schema shapes', () => {
    test('primitive union variants are named by their type', () => {
        const section = schemaSection(
            renderSchemas(reference({ Mixed: { oneOf: [{ type: 'string' }, { type: 'number', format: 'double' }] } })),
            'Mixed',
        )
        expect(section).toContain('| string |')
        expect(section).toContain('| number (double) |')
        expect(section).not.toContain('Variant 1')
    })

    test('a nested anyOf variant renders its members', () => {
        const section = schemaSection(
            renderSchemas(
                reference({
                    Nested: {
                        oneOf: [
                            { anyOf: [{ type: 'string' }, { type: 'integer' }] },
                            { type: 'object', required: ['a'], properties: { a: { type: 'boolean' } } },
                        ],
                    },
                }),
            ),
            'Nested',
        )
        expect(section).toContain('string or integer')
        expect(section).toContain('`a`')
    })

    test('an unsupported schema keyword fails generation', () => {
        for (const keyword of ['not', 'prefixItems', 'patternProperties', 'if']) {
            expect(() => reference({ Odd: { type: 'object', [keyword]: {} } })).toThrow(
                new RegExp(`unsupported schema keyword \`${keyword}\``),
            )
        }
    })

    test('an unsupported keyword nested in a property fails generation', () => {
        expect(() =>
            reference({ Odd: { type: 'object', properties: { inner: { type: 'array', prefixItems: [] } } } }),
        ).toThrow(/Odd\.properties\.inner/)
    })

    test('an allOf member that is not an object or reference fails generation', () => {
        const schemas = {
            Base: { type: 'object', properties: { id: { type: 'string' } } },
            Odd: { allOf: [{ $ref: '#/components/schemas/Base' }, { type: 'string' }] },
        }
        expect(() => renderSchemas(reference(schemas))).toThrow(/unsupported allOf member/)
    })
})
