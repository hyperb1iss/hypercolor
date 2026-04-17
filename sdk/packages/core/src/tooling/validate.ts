import { basename } from 'node:path'

import { parseHtmlArtifact } from './html'
import type { ValidationMessage, ValidationResult } from './types'

const VALID_CONTROL_TYPES = new Set([
    'number',
    'boolean',
    'color',
    'combobox',
    'dropdown',
    'hue',
    'text',
    'textfield',
    'input',
    'sensor',
    'area',
    'rect',
])

function warning(check: string, code: string, message: string): ValidationMessage {
    return { check, code, message }
}

function error(check: string, code: string, message: string): ValidationMessage {
    return { check, code, message }
}

export function validateHtmlArtifact(html: string, filePath: string): ValidationResult {
    const parsed = parseHtmlArtifact(html)
    const errors: ValidationMessage[] = []
    const warnings: ValidationMessage[] = []

    if (!parsed.hasRenderSurface) {
        errors.push(error('render_surface', 'MISSING_RENDER_SURFACE', 'Missing required render surface'))
    }
    if (!parsed.title) {
        errors.push(error('title', 'MISSING_TITLE', 'Missing <title> tag'))
    }
    if (!parsed.hasScript) {
        errors.push(error('script', 'MISSING_SCRIPT', 'Missing <script> tag'))
    }
    if (!parsed.version) {
        warnings.push(warning('format_version', 'MISSING_VERSION', 'Missing hypercolor-version meta tag'))
    }
    if (!parsed.description) {
        warnings.push(warning('description', 'MISSING_DESCRIPTION', 'Missing description metadata'))
    }
    if (!parsed.publisher) {
        warnings.push(warning('publisher', 'MISSING_PUBLISHER', 'Missing publisher metadata'))
    }

    const seenControls = new Set<string>()
    for (const control of parsed.controls) {
        if (!VALID_CONTROL_TYPES.has(control.type)) {
            errors.push(
                error(
                    'control_type',
                    'INVALID_CONTROL_TYPE',
                    `Control "${control.property}" uses unknown type "${control.type}"`,
                ),
            )
        }
        if (seenControls.has(control.property)) {
            errors.push(
                error('control_ids', 'DUPLICATE_CONTROL_ID', `Duplicate control property "${control.property}"`),
            )
        } else {
            seenControls.add(control.property)
        }
        if (control.min != null && control.max != null && control.min >= control.max) {
            errors.push(error('control_range', 'INVALID_CONTROL_RANGE', `Control "${control.property}" has min >= max`))
        }
        if (control.defaultValue != null && control.min != null && control.max != null) {
            const defaultNumber = Number(control.defaultValue)
            if (Number.isFinite(defaultNumber) && (defaultNumber < control.min || defaultNumber > control.max)) {
                warnings.push(
                    warning(
                        'control_default',
                        'DEFAULT_OUT_OF_RANGE',
                        `Control "${control.property}" has default outside its declared range`,
                    ),
                )
            }
        }
        if ((control.type === 'combobox' || control.type === 'dropdown') && control.values.length === 0) {
            errors.push(
                error(
                    'control_values',
                    'MISSING_COMBOBOX_VALUES',
                    `Control "${control.property}" is a combobox without values`,
                ),
            )
        }
    }

    if (parsed.canvasWidth != null && (parsed.canvasWidth < 100 || parsed.canvasWidth > 1920)) {
        warnings.push(
            warning('canvas_size', 'UNUSUAL_CANVAS_WIDTH', `Canvas width ${parsed.canvasWidth} is outside 100-1920`),
        )
    }
    if (parsed.canvasHeight != null && (parsed.canvasHeight < 100 || parsed.canvasHeight > 1920)) {
        warnings.push(
            warning('canvas_size', 'UNUSUAL_CANVAS_HEIGHT', `Canvas height ${parsed.canvasHeight} is outside 100-1920`),
        )
    }

    if (parsed.audioReactive != null && !['true', 'false'].includes(parsed.audioReactive.toLowerCase())) {
        warnings.push(
            warning(
                'audio_meta',
                'INVALID_AUDIO_META',
                `audio-reactive should be "true" or "false", got "${parsed.audioReactive}"`,
            ),
        )
    }

    if (parsed.hasExternalAssets) {
        warnings.push(
            warning(
                'self_contained',
                'EXTERNAL_ASSET_REFERENCE',
                'Effect references external script or link tags and may not be self-contained',
            ),
        )
    }

    for (const preset of parsed.presets) {
        if (preset.parseError) {
            errors.push(
                error(
                    'preset_json',
                    'INVALID_PRESET_JSON',
                    `Preset "${preset.name}" has invalid preset-controls JSON: ${preset.parseError}`,
                ),
            )
            continue
        }

        for (const [controlId, value] of Object.entries(preset.controls)) {
            const control = parsed.controls.find((item) => item.property === controlId)
            if (!control) {
                warnings.push(
                    warning(
                        'preset_refs',
                        'UNKNOWN_PRESET_CONTROL',
                        `Preset "${preset.name}" references unknown control "${controlId}"`,
                    ),
                )
                continue
            }

            if ((control.type === 'combobox' || control.type === 'dropdown') && !control.values.includes(value)) {
                warnings.push(
                    warning(
                        'preset_values',
                        'INVALID_PRESET_COMBOBOX_VALUE',
                        `Preset "${preset.name}" uses value "${value}" not found in control "${controlId}" options`,
                    ),
                )
            }
        }
    }

    return {
        errors,
        file: basename(filePath),
        metadata: {
            audioReactive: parsed.audioReactive?.toLowerCase() === 'true',
            controls: parsed.controls.length,
            presets: parsed.presets.length,
            title: parsed.title ?? 'Unnamed Effect',
        },
        valid: errors.length === 0,
        warnings,
    }
}

export async function validateHtmlArtifactFile(filePath: string): Promise<ValidationResult> {
    const html = await Bun.file(filePath).text()
    return validateHtmlArtifact(html, filePath)
}
