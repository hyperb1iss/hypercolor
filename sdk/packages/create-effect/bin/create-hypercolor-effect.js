#!/usr/bin/env node

if (typeof Bun === 'undefined') {
    console.error('create-hypercolor-effect requires Bun. Install Bun from https://bun.sh and try again.')
    process.exit(1)
}

const cli = await import('../src/cli.ts')
const exitCode = await cli.main(process.argv.slice(2))
process.exit(typeof exitCode === 'number' ? exitCode : 0)
