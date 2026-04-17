import { afterEach, describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { installArtifactsLocally, installArtifactsViaDaemon } from '../src/tooling'

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
    const servers: Bun.Server[] = []

    afterEach(() => {
        while (servers.length > 0) {
            servers.pop()?.stop(true)
        }
    })

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

    test('uploads validated artifacts to the daemon install endpoint', async () => {
        const workspace = mkdtempSync(join(tmpdir(), 'hypercolor-install-daemon-'))
        const distDir = join(workspace, 'dist')
        const artifactPath = join(distDir, 'aurora.html')

        mkdirSync(distDir, { recursive: true })
        writeFileSync(artifactPath, VALID_HTML)

        let uploadedFileName = ''
        let uploadedTitle = ''
        const server = Bun.serve({
            fetch: async (request) => {
                expect(request.method).toBe('POST')
                expect(new URL(request.url).pathname).toBe('/api/v1/effects/install')
                const formData = await request.formData()
                const file = formData.get('file')
                expect(file).toBeInstanceOf(File)
                uploadedFileName = (file as File).name
                uploadedTitle = await (file as File).text()

                return Response.json({
                    data: {
                        controls: 3,
                        id: 'test-id',
                        name: 'Aurora',
                        path: '/tmp/user-effects/aurora.html',
                        presets: 1,
                        source: 'user',
                    },
                    meta: {
                        api_version: '1.0',
                        request_id: 'req_test',
                        timestamp: '2026-04-17T00:00:00.000Z',
                    },
                })
            },
            port: 0,
        })
        servers.push(server)

        try {
            const result = await installArtifactsViaDaemon({
                cwd: workspace,
                daemonUrl: `http://127.0.0.1:${server.port}`,
            })

            expect(result.failures).toHaveLength(0)
            expect(result.successes).toHaveLength(1)
            expect(result.successes[0]?.installedPath).toBe('/tmp/user-effects/aurora.html')
            expect(result.successes[0]?.installedName).toBe('Aurora')
            expect(result.successes[0]?.source).toBe('daemon')
            expect(uploadedFileName).toBe('aurora.html')
            expect(uploadedTitle).toContain('<title>Aurora</title>')
        } finally {
            rmSync(workspace, { force: true, recursive: true })
        }
    })
})
