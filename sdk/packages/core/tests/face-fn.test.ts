import { describe, expect, test } from 'bun:test'

import { combo, font } from '../src/controls/specs'
import type { FaceContext } from '../src/faces/context'
import { __testing, face } from '../src/faces/face-fn'

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

describe('face audio opt-in', () => {
    function withMetadataOnly(run: () => void): unknown[] {
        Reflect.set(globalThis, '__HYPERCOLOR_METADATA_ONLY__', true)
        Reflect.deleteProperty(globalThis, '__hypercolorEffectDefs__')
        try {
            run()
            return (Reflect.get(globalThis, '__hypercolorEffectDefs__') as unknown[]) ?? []
        } finally {
            Reflect.deleteProperty(globalThis, '__HYPERCOLOR_METADATA_ONLY__')
            Reflect.deleteProperty(globalThis, '__hypercolorEffectDefs__')
        }
    }

    test('audio: true is carried into registered metadata', () => {
        const defs = withMetadataOnly(() => {
            face('Audio Probe', {}, { audio: true }, () => () => {})
        })

        expect(defs).toHaveLength(1)
        expect((defs[0] as { audio?: boolean }).audio).toBe(true)
    })

    test('audio defaults to false when not requested', () => {
        const defs = withMetadataOnly(() => {
            face('Quiet Probe', {}, {}, () => () => {})
        })

        expect(defs).toHaveLength(1)
        expect((defs[0] as { audio?: boolean }).audio).toBe(false)
    })

    test('update functions receive a silent-safe audio accessor', () => {
        const calls: string[] = []
        const originalCaptureMode = Reflect.get(globalThis, '__hypercolorCaptureMode')
        const originalHostDriven = Reflect.get(globalThis, '__hypercolorHostDrivenAnimation')
        const originalWindow = Reflect.get(globalThis, 'window')
        const host = { performance: { now: () => 1000 } }

        Reflect.set(globalThis, '__hypercolorCaptureMode', true)
        Reflect.set(globalThis, '__hypercolorHostDrivenAnimation', true)
        Reflect.set(globalThis, 'window', host)

        try {
            __testing.startFaceLoop(
                testAudioContext(calls),
                () => (_time, _controls, _sensors, audio) => {
                    calls.push(`available:${audio.available()}`)
                    calls.push(`level:${audio.data().level}`)
                    calls.push(`mel:${audio.data().melBands.length}`)
                },
                [],
                [],
            )

            expect(calls).toEqual(['clear', 'available:false', 'level:0', 'mel:24'])
        } finally {
            if (originalCaptureMode === undefined) Reflect.deleteProperty(globalThis, '__hypercolorCaptureMode')
            else Reflect.set(globalThis, '__hypercolorCaptureMode', originalCaptureMode)
            if (originalHostDriven === undefined) Reflect.deleteProperty(globalThis, '__hypercolorHostDrivenAnimation')
            else Reflect.set(globalThis, '__hypercolorHostDrivenAnimation', originalHostDriven)
            if (originalWindow === undefined) Reflect.deleteProperty(globalThis, 'window')
            else Reflect.set(globalThis, 'window', originalWindow)
        }
    })

    function testAudioContext(calls: string[]): FaceContext {
        return {
            canvas: {} as HTMLCanvasElement,
            circular: false,
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
})
