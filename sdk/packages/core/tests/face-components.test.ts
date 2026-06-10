import { afterEach, beforeEach, describe, expect, test } from 'bun:test'

import {
    createChartPanel,
    createMetricCard,
    createProgressBar,
    createReadout,
} from '../../../src/faces/shared/components'

// ── Minimal DOM stub ────────────────────────────────────────────────────

interface StubElement {
    tagName: string
    className: string
    children: StubElement[]
    style: Record<string, string>
    textContent: string | null
    width: number
    height: number
    appendChild(child: StubElement): StubElement
    getContext(kind: string): unknown
    __ctxOps: Array<{ op: string; args: unknown[] }>
}

function stubElement(tagName: string): StubElement {
    const element: StubElement = {
        __ctxOps: [],
        appendChild(child) {
            element.children.push(child)
            return child
        },
        children: [],
        className: '',
        getContext() {
            const record =
                (op: string) =>
                (...args: unknown[]) => {
                    element.__ctxOps.push({ args, op })
                }
            return {
                beginPath: record('beginPath'),
                clearRect: record('clearRect'),
                closePath: record('closePath'),
                createLinearGradient: () => ({ addColorStop: () => {} }),
                fill: record('fill'),
                fillStyle: '',
                lineCap: 'round',
                lineJoin: 'round',
                lineTo: record('lineTo'),
                lineWidth: 1,
                moveTo: record('moveTo'),
                stroke: record('stroke'),
                strokeStyle: '',
            }
        },
        height: 0,
        style: {},
        tagName: tagName.toUpperCase(),
        textContent: null,
        width: 0,
    }
    return element
}

let originalDocument: unknown

beforeEach(() => {
    originalDocument = Reflect.get(globalThis, 'document')
    Reflect.set(globalThis, 'document', {
        createElement: (tag: string) => stubElement(tag),
    })
})

afterEach(() => {
    if (originalDocument === undefined) {
        Reflect.deleteProperty(globalThis, 'document')
    } else {
        Reflect.set(globalThis, 'document', originalDocument)
    }
})

function parent(): HTMLElement {
    return stubElement('div') as unknown as HTMLElement
}

const CELL = { height: 100, width: 200, x: 20, y: 40 }

// ── Tests ───────────────────────────────────────────────────────────────

describe('createReadout', () => {
    test('builds label and value and places into a rect', () => {
        const host = parent()
        const readout = createReadout(host, { label: 'CPU' })
        readout.place(CELL)

        const root = (host as unknown as StubElement).children[0]
        expect(root?.className).toBe('hc-readout')
        expect(root?.children[0]?.textContent).toBe('CPU')
        expect(root?.children[1]?.textContent).toBe('--')
        expect(root?.style.left).toBe('20px')
        expect(root?.style.width).toBe('200px')
    })

    test('update writes only on change', () => {
        const host = parent()
        const readout = createReadout(host, { label: 'CPU' })
        readout.update('64°C')

        const value = (host as unknown as StubElement).children[0]?.children[1]
        expect(value?.textContent).toBe('64°C')

        readout.setLabel('GPU')
        const label = (host as unknown as StubElement).children[0]?.children[0]
        expect(label?.textContent).toBe('GPU')
    })
})

describe('createProgressBar', () => {
    test('eases toward the target instead of jumping', () => {
        const host = parent()
        const bar = createProgressBar(host, { halflife: 0.1 })

        bar.update(1, 0.1)
        expect(bar.value()).toBeCloseTo(0.5)
        bar.update(1, 0.1)
        expect(bar.value()).toBeCloseTo(0.75)

        const fill = (host as unknown as StubElement).children[0]?.children[0]
        expect(fill?.style.width).toBe('75%')
    })

    test('place vertically centers the track in its cell', () => {
        const host = parent()
        const bar = createProgressBar(host, { height: 6 })
        bar.place(CELL)

        const track = (host as unknown as StubElement).children[0]
        expect(track?.style.top).toBe('87px')
        expect(track?.style.height).toBe('6px')
    })
})

describe('createMetricCard', () => {
    test('composes readout and bar, updates both', () => {
        const host = parent()
        const card = createMetricCard(host, { halflife: 0.1, label: 'CPU Temp' })
        card.place(CELL)
        card.update({ dt: 0.1, normalized: 1, text: '72°C' })

        const root = (host as unknown as StubElement).children[0]
        expect(root?.className).toBe('hc-metric-card')
        const readout = root?.children[0]
        expect(readout?.children[1]?.textContent).toBe('72°C')
        expect(card.barValue()).toBeCloseTo(0.5)
    })

    test('sparkline option draws into a backing chart', () => {
        const host = parent()
        const card = createMetricCard(host, { label: 'Load', sparkline: true })
        card.place(CELL)
        for (let i = 0; i < 5; i++) {
            card.update({ dt: 1 / 30, normalized: i / 4, text: `${i}` })
        }

        const root = (host as unknown as StubElement).children[0]
        const chart = root?.children.find((child) => child.tagName === 'CANVAS')
        expect(chart).toBeDefined()
        expect(chart?.__ctxOps.some((op) => op.op === 'stroke')).toBe(true)
    })

    test('bar can be disabled', () => {
        const host = parent()
        createMetricCard(host, { bar: false, label: 'Net' })
        const root = (host as unknown as StubElement).children[0]
        expect(root?.children.some((child) => child.className === 'hc-progress')).toBe(false)
    })
})

describe('createChartPanel', () => {
    test('resizes with placement and draws history', () => {
        const host = parent()
        const panel = createChartPanel(host, { color: '#80ffea', range: [0, 100] })
        panel.place(CELL)

        const canvas = (host as unknown as StubElement).children[0]
        expect(canvas?.width).toBe(200)
        expect(canvas?.height).toBe(100)

        panel.push(10)
        panel.draw()
        expect(canvas?.__ctxOps.length ?? 0).toBe(0)

        panel.push(60)
        panel.push(90)
        panel.draw()
        expect(canvas?.__ctxOps.some((op) => op.op === 'stroke')).toBe(true)
    })
})
