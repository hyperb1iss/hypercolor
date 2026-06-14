import { describe, expect, test } from 'bun:test'

import { __testing, BaseEffect } from '../src/effects/base-effect'

class TestEffect extends BaseEffect<Record<string, never>> {
    public readonly renders: number[] = []
    public updates = 0

    public begin(): void {
        this.startAnimation()
    }

    public end(): void {
        this.stop()
    }

    protected async initializeRenderer(): Promise<void> {}

    protected render(time: number): void {
        this.renders.push(time)
    }

    protected initializeControls(): void {}

    protected getControlValues(): Record<string, never> {
        return {}
    }

    protected updateParameters(): void {
        this.updates += 1
    }
}

function restoreGlobal(name: string, value: unknown): void {
    if (value === undefined) {
        Reflect.deleteProperty(globalThis, name)
        return
    }

    Reflect.set(globalThis, name, value)
}

describe('BaseEffect FPS cap cadence', () => {
    test('accepts near-boundary frames at 30fps', () => {
        const next = __testing.nextFpsCapFrameTime(33.32, 0, 30)

        expect(next).toBeCloseTo(1000 / 30, 6)
    })

    test('keeps cadence aligned after delayed frames', () => {
        const interval = 1000 / 30
        const first = __testing.nextFpsCapFrameTime(33.32, 0, 30)
        const second = __testing.nextFpsCapFrameTime(83.4, first ?? 0, 30)

        expect(second).toBeCloseTo(interval * 2, 6)
    })

    test('skips genuinely early frames', () => {
        expect(__testing.nextFpsCapFrameTime(16.67, 0, 30)).toBeNull()
    })
})

describe('BaseEffect host-driven capture loop', () => {
    test('renders through the host hook without scheduling raf', () => {
        const originalWindow = Reflect.get(globalThis, 'window')
        const originalRaf = Reflect.get(globalThis, 'requestAnimationFrame')
        let rafCalls = 0
        const host = {
            __hypercolorCaptureMode: true,
            __hypercolorHostDrivenAnimation: true,
            performance: { now: () => 1234 },
        }

        Reflect.set(globalThis, 'window', host)
        Reflect.set(globalThis, 'requestAnimationFrame', () => {
            rafCalls += 1
            return 7
        })

        try {
            const effect = new TestEffect({ id: 'test', name: 'Test' })
            effect.begin()

            expect(rafCalls).toBe(0)
            expect(effect.renders).toEqual([1.234])
            expect(effect.updates).toBe(1)
            expect(Reflect.get(host, 'currentAnimationFrame')).toBeUndefined()

            const renderHostFrame = Reflect.get(host, '__hypercolorRenderHostFrame')
            expect(typeof renderHostFrame).toBe('function')
            ;(renderHostFrame as () => void)()
            expect(effect.renders).toEqual([1.234, 1.234])
            expect(effect.updates).toBe(1)

            effect.end()
            expect(Reflect.get(host, '__hypercolorRenderHostFrame')).toBeUndefined()
            expect(Reflect.get(host, 'effectInstance')).toBeUndefined()
        } finally {
            restoreGlobal('window', originalWindow)
            restoreGlobal('requestAnimationFrame', originalRaf)
        }
    })

    test('browser mode keeps requestAnimationFrame in charge', () => {
        const originalWindow = Reflect.get(globalThis, 'window')
        const originalRaf = Reflect.get(globalThis, 'requestAnimationFrame')
        let rafCallback: ((timestamp: number) => void) | undefined
        let rafCalls = 0
        const host = {
            __hypercolorCaptureMode: false,
            __hypercolorHostDrivenAnimation: false,
            performance: { now: () => 1234 },
        }

        Reflect.set(globalThis, 'window', host)
        Reflect.set(globalThis, 'requestAnimationFrame', (callback: (timestamp: number) => void) => {
            rafCalls += 1
            rafCallback = callback
            return 7
        })

        try {
            const effect = new TestEffect({ id: 'test', name: 'Test' })
            effect.begin()

            expect(rafCalls).toBe(1)
            expect(effect.renders).toEqual([])
            expect(Reflect.get(host, 'currentAnimationFrame')).toBe(7)

            rafCallback?.(2500)
            expect(rafCalls).toBe(2)
            expect(effect.renders).toEqual([2.5])
        } finally {
            restoreGlobal('window', originalWindow)
            restoreGlobal('requestAnimationFrame', originalRaf)
        }
    })

    test('late host marker cancels a pending raf before host render', () => {
        const originalWindow = Reflect.get(globalThis, 'window')
        const originalRaf = Reflect.get(globalThis, 'requestAnimationFrame')
        const originalCancelRaf = Reflect.get(globalThis, 'cancelAnimationFrame')
        let rafCallback: ((timestamp: number) => void) | undefined
        const canceledFrames: number[] = []
        const host = {
            __hypercolorCaptureMode: false,
            __hypercolorHostDrivenAnimation: false,
            performance: { now: () => 1234 },
        }

        Reflect.set(globalThis, 'window', host)
        Reflect.set(globalThis, 'requestAnimationFrame', (callback: (timestamp: number) => void) => {
            rafCallback = callback
            return 7
        })
        Reflect.set(globalThis, 'cancelAnimationFrame', (frame: number) => {
            canceledFrames.push(frame)
        })

        try {
            const effect = new TestEffect({ id: 'test', name: 'Test' })
            effect.begin()

            Reflect.set(host, '__hypercolorCaptureMode', true)
            Reflect.set(host, '__hypercolorHostDrivenAnimation', true)
            const renderHostFrame = Reflect.get(host, '__hypercolorRenderHostFrame') as () => void
            renderHostFrame()

            expect(canceledFrames).toEqual([7])
            expect(Reflect.get(host, 'currentAnimationFrame')).toBeUndefined()
            expect(effect.renders).toEqual([1.234])

            rafCallback?.(2500)
            expect(effect.renders).toEqual([1.234])
        } finally {
            restoreGlobal('window', originalWindow)
            restoreGlobal('requestAnimationFrame', originalRaf)
            restoreGlobal('cancelAnimationFrame', originalCancelRaf)
        }
    })
})
