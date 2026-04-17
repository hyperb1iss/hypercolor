import { describe, expect, test } from 'bun:test'

import { HYPERCOLOR_FORMAT_VERSION, validateHtmlArtifact } from '../src/tooling'

const VALID_EFFECT = `<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="hypercolor-version" content="${HYPERCOLOR_FORMAT_VERSION}" />
    <title>Aurora</title>
    <meta description="Good effect" />
    <meta publisher="Hypercolor" />
    <meta property="speed" label="Speed" type="number" min="1" max="10" default="5" />
    <meta property="palette" label="Palette" type="combobox" values="Aurora,Fire" default="Aurora" />
    <meta preset="Calm" preset-controls='{"speed":"2","palette":"Aurora"}' />
  </head>
  <body>
    <canvas id="exCanvas"></canvas>
    <script>console.log('ok')</script>
  </body>
</html>`

describe('tooling validate', () => {
    test('accepts a valid effect artifact', () => {
        const result = validateHtmlArtifact(VALID_EFFECT, '/tmp/aurora.html')

        expect(result.valid).toBeTrue()
        expect(result.errors).toHaveLength(0)
        expect(result.warnings).toHaveLength(0)
        expect(result.metadata.controls).toBe(2)
        expect(result.metadata.presets).toBe(1)
    })

    test('flags duplicate controls and invalid preset JSON', () => {
        const html = VALID_EFFECT.replace(
            '</head>',
            `
    <meta property="speed" label="Speed Again" type="number" min="1" max="10" default="5" />
    <meta preset="Broken" preset-controls='{"speed":' />
  </head>`,
        )

        const result = validateHtmlArtifact(html, '/tmp/broken.html')

        expect(result.valid).toBeFalse()
        expect(result.errors.some((entry) => entry.code === 'DUPLICATE_CONTROL_ID')).toBeTrue()
        expect(result.errors.some((entry) => entry.code === 'INVALID_PRESET_JSON')).toBeTrue()
    })

    test('warns for missing version and unknown preset controls', () => {
        const html = VALID_EFFECT.replace('<meta name="hypercolor-version" content="1" />\n', '').replace(
            `{"speed":"2","palette":"Aurora"}`,
            `{"speed":"2","ghost":"oops"}`,
        )

        const result = validateHtmlArtifact(html, '/tmp/warn.html')

        expect(result.valid).toBeTrue()
        expect(result.warnings.some((entry) => entry.code === 'MISSING_VERSION')).toBeTrue()
        expect(result.warnings.some((entry) => entry.code === 'UNKNOWN_PRESET_CONTROL')).toBeTrue()
    })
})
