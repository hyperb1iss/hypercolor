import { describe, expect, test } from 'bun:test'

import { combo, font } from '../src/controls/specs'
import type { FaceContext } from '../src/faces/context'
import { __testing } from '../src/faces/face-fn'

describe('face font loading', () => {
    test('loads only the active font selections', () => {
        const controls = __testing.resolveFaceControls({
            heroFont: font('Hero Font', 'Rajdhani', { families: ['Rajdhani', 'Orbitron', 'Space Mono'] }),
            meterStyle: combo('Meter Style', ['Halo', 'Vector', 'Scope']),
            uiFont: font('UI Font', 'Inter', { families: ['Inter', 'DM Sans', 'Sora'] }),
        })

        const fontControls = __testing.resolveFaceFontControls(controls)
        const families = __testing.resolveFaceFontFamilies(fontControls, {
            heroFont: 'Orbitron',
            meterStyle: 'Scope',
            uiFont: 'DM Sans',
        })

        expect(families).toEqual(['Orbitron', 'DM Sans'])
    })

    test('falls back to defaults and dedupes shared fonts', () => {
        const controls = __testing.resolveFaceControls({
            headlineFont: font('Headline Font', 'Rajdhani', { families: ['Rajdhani', 'Orbitron'] }),
            uiFont: font('UI Font', 'Rajdhani', { families: ['Rajdhani', 'Inter'] }),
        })

        const fontControls = __testing.resolveFaceFontControls(controls)
        const families = __testing.resolveFaceFontFamilies(fontControls, {
            headlineFont: '',
            uiFont: 'Rajdhani',
        })

        expect(families).toEqual(['Rajdhani'])
    })

    test('skips remote font loading in capture mode', () => {
        Reflect.set(globalThis, '__hypercolorCaptureMode', true)
        Reflect.set(globalThis, '__hypercolorHostDrivenAnimation', true)

        try {
            expect(__testing.shouldLoadRemoteFaceFonts()).toBe(false)
            expect(__testing.shouldUseHostDrivenFaceLoop()).toBe(true)
        } finally {
            Reflect.deleteProperty(globalThis, '__hypercolorCaptureMode')
            Reflect.deleteProperty(globalThis, '__hypercolorHostDrivenAnimation')
        }
    })

    test('does not use host driven rendering without the host marker', () => {
        Reflect.set(globalThis, '__hypercolorCaptureMode', true)

        try {
            expect(__testing.shouldLoadRemoteFaceFonts()).toBe(false)
            expect(__testing.shouldUseHostDrivenFaceLoop()).toBe(false)
        } finally {
            Reflect.deleteProperty(globalThis, '__hypercolorCaptureMode')
        }
    })
})

describe('face render loop', () => {
    function testContext(calls: string[]): FaceContext {
        return {
            canvas: {} as HTMLCanvasElement,
            circular: true,
            container: {} as HTMLDivElement,
            ctx: {
                clearRect: () => calls.push('clear'),
            } as unknown as CanvasRenderingContext2D,
            dpr: 1,
            height: 10,
            scale: 1,
            width: 10,
        }
    }

    function restoreGlobal(name: string, value: unknown): void {
        if (value === undefined) {
            Reflect.deleteProperty(globalThis, name)
        } else {
            Reflect.set(globalThis, name, value)
        }
    }

    test('capture mode renders once through the host hook without scheduling raf', () => {
        const calls: string[] = []
        const originalCaptureMode = Reflect.get(globalThis, '__hypercolorCaptureMode')
        const originalHostDriven = Reflect.get(globalThis, '__hypercolorHostDrivenAnimation')
        const originalRaf = Reflect.get(globalThis, 'requestAnimationFrame')
        const originalWindow = Reflect.get(globalThis, 'window')
        let rafCalls = 0
        const host = { performance: { now: () => 1234 } }

        Reflect.set(globalThis, '__hypercolorCaptureMode', true)
        Reflect.set(globalThis, '__hypercolorHostDrivenAnimation', true)
        Reflect.set(globalThis, 'requestAnimationFrame', () => {
            rafCalls += 1
            return 7
        })
        Reflect.set(globalThis, 'window', host)

        try {
            __testing.startFaceLoop(testContext(calls), () => (time) => calls.push(`update:${time.toFixed(3)}`), [], [])

            expect(rafCalls).toBe(0)
            expect(calls).toEqual(['clear', 'update:1.234'])

            const renderHostFrame = Reflect.get(host, '__hypercolorRenderHostFrame')
            expect(typeof renderHostFrame).toBe('function')
            const renderHostFrameFn = renderHostFrame as () => void
            renderHostFrameFn()
            expect(calls).toEqual(['clear', 'update:1.234', 'clear', 'update:1.234'])
        } finally {
            restoreGlobal('__hypercolorCaptureMode', originalCaptureMode)
            restoreGlobal('__hypercolorHostDrivenAnimation', originalHostDriven)
            restoreGlobal('requestAnimationFrame', originalRaf)
            restoreGlobal('window', originalWindow)
        }
    })

    test('browser mode keeps requestAnimationFrame in charge', () => {
        const calls: string[] = []
        const originalCaptureMode = Reflect.get(globalThis, '__hypercolorCaptureMode')
        const originalHostDriven = Reflect.get(globalThis, '__hypercolorHostDrivenAnimation')
        const originalRaf = Reflect.get(globalThis, 'requestAnimationFrame')
        const originalWindow = Reflect.get(globalThis, 'window')
        let rafCallback: ((timestamp: number) => void) | undefined
        let rafCalls = 0
        const host = { performance: { now: () => 1234 } }

        Reflect.deleteProperty(globalThis, '__hypercolorCaptureMode')
        Reflect.deleteProperty(globalThis, '__hypercolorHostDrivenAnimation')
        Reflect.set(globalThis, 'requestAnimationFrame', (callback: (timestamp: number) => void) => {
            rafCalls += 1
            rafCallback = callback
            return 7
        })
        Reflect.set(globalThis, 'window', host)

        try {
            __testing.startFaceLoop(testContext(calls), () => (time) => calls.push(`update:${time.toFixed(3)}`), [], [])

            expect(rafCalls).toBe(1)
            expect(calls).toEqual([])

            rafCallback?.(2500)
            expect(rafCalls).toBe(2)
            expect(calls).toEqual(['clear', 'update:2.500'])
        } finally {
            restoreGlobal('__hypercolorCaptureMode', originalCaptureMode)
            restoreGlobal('__hypercolorHostDrivenAnimation', originalHostDriven)
            restoreGlobal('requestAnimationFrame', originalRaf)
            restoreGlobal('window', originalWindow)
        }
    })
})
