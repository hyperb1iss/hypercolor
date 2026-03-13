/**
 * Effect initialization — lifecycle management for both old and new APIs.
 */

export type InitializationMode = 'immediate' | 'deferred' | 'metadata-only'

export interface InitOptions {
    mode?: InitializationMode
    onReady?: () => void
    instance?: object
}

function detectInitMode(): InitializationMode {
    if (typeof window === 'undefined') return 'metadata-only'
    if ((window as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__) return 'metadata-only'
    return 'immediate'
}

/**
 * Initialize a Hypercolor effect with proper lifecycle handling.
 * In metadata-only mode, stores the effect instance for build-time extraction.
 */
export function initializeEffect(initFunction: () => void, options: InitOptions = {}): void {
    const mode = options.mode ?? detectInitMode()
    if (mode === 'metadata-only') {
        // Store instance for metadata extraction by build tools
        if (options.instance) {
            ;(globalThis as any).__hypercolorEffectInstance__ = options.instance
        }
        return
    }

    if (document.readyState === 'complete' || document.readyState === 'interactive') {
        initFunction()
        options.onReady?.()
        return
    }

    window.addEventListener(
        'DOMContentLoaded',
        () => {
            if ((window as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__) return
            initFunction()
            options.onReady?.()
        },
        { once: true },
    )
}
