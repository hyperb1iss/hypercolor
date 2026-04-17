import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { installArtifactsLocally } from '../src/tooling'

const VALID_HTML = `<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="hypercolor-version" content="1" />
    <title>Aurora</title>
    <meta description="Good effect" />
    <meta publisher="Hypercolor" />
  </head>
  <body>
    <canvas id="exCanvas"></canvas>
    <script>console.log('ok')</script>
  </body>
</html>`

describe('tooling install', () => {
    test('copies validated artifacts into the user effects directory', async () => {
        const workspace = mkdtempSync(join(tmpdir(), 'hypercolor-install-'))
        const distDir = join(workspace, 'dist')
        const userEffectsDir = join(workspace, 'user-effects')
        const artifactPath = join(distDir, 'aurora.html')

        mkdirSync(distDir, { recursive: true })
        writeFileSync(artifactPath, VALID_HTML)

        try {
            const result = await installArtifactsLocally({
                cwd: workspace,
                userEffectsDir,
            })

            expect(result.failures).toHaveLength(0)
            expect(result.successes).toHaveLength(1)
            expect(result.successes[0]?.installedPath).toBe(join(userEffectsDir, 'aurora.html'))
        } finally {
            rmSync(workspace, { force: true, recursive: true })
        }
    })

    test('continues past invalid files and reports failures', async () => {
        const workspace = mkdtempSync(join(tmpdir(), 'hypercolor-install-fail-'))
        const distDir = join(workspace, 'dist')
        const userEffectsDir = join(workspace, 'user-effects')
        mkdirSync(distDir, { recursive: true })
        writeFileSync(join(distDir, 'good.html'), VALID_HTML)
        writeFileSync(join(distDir, 'bad.html'), '<html><body>nope</body></html>')

        try {
            const result = await installArtifactsLocally({
                cwd: workspace,
                userEffectsDir,
            })

            expect(result.successes).toHaveLength(1)
            expect(result.failures).toHaveLength(1)
            expect(result.failures[0]?.errors.some((message) => message.includes('Missing <title>'))).toBeTrue()
        } finally {
            rmSync(workspace, { force: true, recursive: true })
        }
    })
})
