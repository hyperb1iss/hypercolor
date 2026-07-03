import { describe, expect, it, spyOn } from 'bun:test'
import { resolveControlNames } from '../src/controls/names'
import { num } from '../src/controls/specs'

describe('resolveControlNames speed-range guard', () => {
    it('does not warn for a magic speed control on the canonical [1, 10] range', () => {
        const warn = spyOn(console, 'warn').mockImplementation(() => {})
        try {
            const resolved = resolveControlNames('speed', num('Speed', [1, 10], 5))
            expect(resolved.normalize).toBe('speed')
            expect(warn).not.toHaveBeenCalled()
        } finally {
            warn.mockRestore()
        }
    })

    it('warns when a magic speed control declares a non-[1, 10] range', () => {
        const warn = spyOn(console, 'warn').mockImplementation(() => {})
        try {
            const resolved = resolveControlNames('speed', num('Speed', [0, 100], 40))
            expect(resolved.normalize).toBe('speed')
            expect(warn).toHaveBeenCalledTimes(1)
            expect(String(warn.mock.calls[0]?.[0])).toContain("normalize: 'none'")
        } finally {
            warn.mockRestore()
        }
    })

    it('warns only once per key/range combination', () => {
        const warn = spyOn(console, 'warn').mockImplementation(() => {})
        try {
            resolveControlNames('speed', num('Speed', [0, 77], 40))
            resolveControlNames('speed', num('Speed', [0, 77], 40))
            expect(warn).toHaveBeenCalledTimes(1)
        } finally {
            warn.mockRestore()
        }
    })

    it('does not warn when the control opts out with normalize none', () => {
        const warn = spyOn(console, 'warn').mockImplementation(() => {})
        try {
            const resolved = resolveControlNames('speed', num('Scroll Speed', [-1, 1], 0.18, { normalize: 'none' }))
            expect(resolved.normalize).toBe('none')
            expect(warn).not.toHaveBeenCalled()
        } finally {
            warn.mockRestore()
        }
    })
})
