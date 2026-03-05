/**
 * Debug utility for Hypercolor effects.
 * SilkCircuit-themed console output.
 */

type LogLevel = 'debug' | 'info' | 'warn' | 'error' | 'success'

// SilkCircuit Neon palette
const COLORS = {
    debug: '#9d9d9d',
    error: '#ff6363',
    primary: '#e135ff',
    secondary: '#80ffea',
    success: '#50fa7b',
    warning: '#f1fa8c',
}

const EMOJI: Record<string, string> = {
    debug: '\u{1f50d}',
    effect: '\u{1f3a8}',
    error: '\u{1f525}',
    info: '\u{1f50c}',
    success: '\u{2728}',
    warn: '\u{26a1}',
}

/** Print the Hypercolor startup banner. */
export function printStartupBanner(): void {
    const banner = `background: linear-gradient(90deg, #0d0221 0%, #e135ff 50%, #80ffea 100%);
        color: #f1fa8c; font-weight: bold; padding: 8px 12px; border-radius: 4px;
        font-size: 16px; text-shadow: 0 0 5px #e135ff, 0 0 10px #80ffea;`
    const credit = `color: ${COLORS.secondary}; font-size: 14px; padding: 4px 12px; font-style: italic;`

    console.log('%c \u2726 Hypercolor SDK \u2726 %c by @hyperb1iss', banner, credit)
}

/** Create a namespaced debug logger. */
export function createDebugLogger(namespace: string, enabled = true) {
    return function debug(...args: unknown[]) {
        if (!enabled) return

        const level: LogLevel =
            (args[0] as LogLevel) && ['debug', 'info', 'warn', 'error', 'success'].includes(args[0] as string)
                ? (args.shift() as LogLevel)
                : 'debug'

        stylizedLog(level, namespace, ...args)
    }
}

function stylizedLog(level: LogLevel, namespace: string, ...args: unknown[]): void {
    let color = COLORS.debug
    let emoji = EMOJI.debug
    let method: 'log' | 'warn' | 'error' = 'log'

    switch (level) {
        case 'info':
            color = COLORS.secondary
            emoji = EMOJI.info
            break
        case 'warn':
            color = COLORS.warning
            emoji = EMOJI.warn
            method = 'warn'
            break
        case 'error':
            color = COLORS.error
            emoji = EMOJI.error
            method = 'error'
            break
        case 'success':
            color = COLORS.success
            emoji = EMOJI.success
            break
    }

    console[method](`%c${emoji} [${namespace}]`, `color: ${color}; font-weight: bold;`, ...args)
}

/** Default debug logger. */
export function debug(...args: unknown[]) {
    const level: LogLevel =
        (args[0] as LogLevel) && ['debug', 'info', 'warn', 'error', 'success'].includes(args[0] as string)
            ? (args.shift() as LogLevel)
            : 'debug'

    stylizedLog(level, 'Hypercolor', ...args)
}
