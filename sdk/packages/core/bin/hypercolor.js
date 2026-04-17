#!/usr/bin/env bun

if (!process.versions.bun) {
    console.error('The @hypercolor/sdk CLI requires Bun. Try `bunx hypercolor ...` instead.')
    process.exit(1)
}

const cliUrl = new URL('../dist/cli.js', import.meta.url)

try {
    const cli = await import(cliUrl.href)
    const exitCode = await cli.main(process.argv.slice(2))
    process.exit(typeof exitCode === 'number' ? exitCode : 0)
} catch (error) {
    console.error(error)
    process.exit(1)
}
