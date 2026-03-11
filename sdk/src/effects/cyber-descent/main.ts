/**
 * Cyber Descent — Cyberpunk city flythrough
 *
 * Raymarched cityscape with neon signs, procedural buildings,
 * 8 color palettes, 3 flight modes, and horizontal camera panning.
 *
 * Adapted from Shadertoy: https://www.shadertoy.com/view/wdfGW4
 */

import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

const CITY_STYLES = ['Standard', 'Fast Descent', 'Neon'] as const

const COLOR_PALETTES = [
    'Classic Cyber',
    'Blade Runner',
    'Synthwave',
    'Matrix',
    'Akira Red',
    'Ice',
    'Toxic',
    'Noir',
] as const

export default effect('Cyber Descent', shader, {
    // Style
    cyberpunkMode: combo('City Style', CITY_STYLES),
    colorPalette: combo('Color Palette', COLOR_PALETTES),

    // Animation
    speed: num('Flight Speed', [1, 10], 5),
    zoom: num('Camera Zoom', [1, 10], 5),

    // Camera
    cameraPitch: num('Camera Pitch', [0, 100], 50, { tooltip: 'Vertical look angle (50 = level)' }),
    cameraRoll: num('Camera Roll', [0, 100], 50, { tooltip: 'Banking angle (50 = level)' }),
    cameraYaw: num('Camera Yaw', [0, 100], 50, { tooltip: 'Horizontal turn (50 = forward)' }),

    // Panning
    panSpeed: num('Pan Speed', [0, 100], 30, { tooltip: 'Horizontal weaving speed' }),
    panWidth: num('Pan Width', [0, 100], 40, { tooltip: 'How wide the camera weaves' }),

    // City
    buildingHeight: num('Building Height', [1, 10], 5),
    buildingFill: num('Building Fill', [0, 100], 20, { tooltip: 'Surface glow density for RGB' }),
    neonFlash: num('Neon Flash', [0, 100], 50),
    streetLights: num('Street Lights', [0, 100], 50),

    // Color
    colorIntensity: num('Color Intensity', [0, 100], 50),
    colorSaturation: num('Color Saturation', [0, 100], 50),
    lightIntensity: num('Light Intensity', [0, 100], 50),
    fogDensity: num('Fog Density', [0, 100], 50),
}, {
    description: 'Cyberpunk city flythrough with raymarched buildings, neon signs, and horizontal camera panning',
})
