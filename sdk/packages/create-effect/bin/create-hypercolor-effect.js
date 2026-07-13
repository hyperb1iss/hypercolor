#!/usr/bin/env node

if (typeof Bun === 'undefined') {
    // Invoked through Node (npx/npm create). The CLI imports TypeScript
    // directly, so re-exec this same script under Bun.
    const { spawnSync } = await import('node:child_process')
    const { fileURLToPath } = await import('node:url')
    const self = fileURLToPath(import.meta.url)
    const result = spawnSync('bun', [self, ...process.argv.slice(2)], { stdio: 'inherit' })
    if (result.error) {
        console.error('create-hypercolor-effect requires Bun. Install Bun from https://bun.sh and try again.')
        process.exit(1)
    }
    process.exit(result.status ?? 1)
}

const cli = await import('../src/cli.ts')
const exitCode = await cli.main(process.argv.slice(2))
process.exit(typeof exitCode === 'number' ? exitCode : 0)
