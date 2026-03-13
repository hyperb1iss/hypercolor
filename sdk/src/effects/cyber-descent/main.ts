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

const CITY_STYLES = ['Fast Descent', 'Neon', 'Standard'] as const

const COLOR_PALETTES = ['Akira Red', 'Blade Runner', 'Classic Cyber', 'Ice', 'Matrix', 'Noir', 'Synthwave', 'Toxic'] as const

export default effect('Cyber Descent', shader, {
    cyberpunkMode: combo('City Style', CITY_STYLES, { group: 'Scene' }),
    colorPalette: combo('Color Palette', COLOR_PALETTES, { group: 'Scene' }),

    speed: num('Flight Speed', [1, 10], 5, { group: 'Camera' }),
    zoom: num('Camera Zoom', [1, 10], 5, { group: 'Camera' }),
    cameraPitch: num('Camera Pitch', [0, 100], 50, { tooltip: 'Vertical look angle (50 = level)', group: 'Camera' }),
    cameraRoll: num('Camera Roll', [0, 100], 50, { tooltip: 'Banking angle (50 = level)', group: 'Camera' }),
    cameraYaw: num('Camera Yaw', [0, 100], 50, { tooltip: 'Horizontal turn (50 = forward)', group: 'Camera' }),
    panSpeed: num('Pan Speed', [0, 100], 30, { tooltip: 'Horizontal weaving speed', group: 'Camera' }),
    panWidth: num('Pan Width', [0, 100], 40, { tooltip: 'How wide the camera weaves', group: 'Camera' }),

    buildingHeight: num('Building Height', [1, 10], 5, { group: 'City' }),
    buildingFill: num('Building Fill', [0, 100], 20, { tooltip: 'Surface glow density for RGB', group: 'City' }),
    rgbSmoothing: num('RGB Smoothing', [0, 100], 60, { tooltip: 'Softens thin building detail for LED layouts', group: 'City' }),
    neonFlash: num('Neon Flash', [0, 100], 50, { group: 'City' }),
    streetLights: num('Street Lights', [0, 100], 50, { group: 'City' }),

    colorIntensity: num('Color Intensity', [0, 100], 50, { group: 'Color' }),
    colorSaturation: num('Color Saturation', [0, 100], 50, { group: 'Color' }),
    lightIntensity: num('Light Intensity', [0, 100], 50, { group: 'Color' }),
    fogDensity: num('Fog Density', [0, 100], 50, { group: 'Color' }),
}, {
    description: 'Cyberpunk city flythrough with raymarched buildings, neon signs, and horizontal camera panning',
    presets: [
        {
            name: 'Neo-Tokyo Express',
            description: 'Bullet-train POV through the Akira sprawl — red neon screaming past at terminal velocity, buildings a smear of crimson and chrome',
            controls: {
                cyberpunkMode: 'Fast Descent',
                colorPalette: 'Akira Red',
                speed: 9,
                zoom: 3,
                cameraPitch: 62,
                cameraRoll: 54,
                cameraYaw: 50,
                panSpeed: 65,
                panWidth: 70,
                buildingHeight: 8,
                buildingFill: 35,
                rgbSmoothing: 45,
                neonFlash: 82,
                streetLights: 70,
                colorIntensity: 78,
                colorSaturation: 72,
                lightIntensity: 65,
                fogDensity: 30,
            },
        },
        {
            name: 'Tyrell Tower Approach',
            description: 'Slow vertical ascent through perpetual rain — Blade Runner amber cutting through fog, pyramidal towers materializing from the murk',
            controls: {
                cyberpunkMode: 'Standard',
                colorPalette: 'Blade Runner',
                speed: 3,
                zoom: 7,
                cameraPitch: 35,
                cameraRoll: 50,
                cameraYaw: 48,
                panSpeed: 15,
                panWidth: 22,
                buildingHeight: 10,
                buildingFill: 12,
                rgbSmoothing: 80,
                neonFlash: 28,
                streetLights: 35,
                colorIntensity: 55,
                colorSaturation: 40,
                lightIntensity: 38,
                fogDensity: 85,
            },
        },
        {
            name: 'Dead Channel Static',
            description: 'The sky above the port — colorless noir cityscape dissolving into signal noise, monochrome towers fading at the edge of perception',
            controls: {
                cyberpunkMode: 'Standard',
                colorPalette: 'Noir',
                speed: 4,
                zoom: 5,
                cameraPitch: 50,
                cameraRoll: 50,
                cameraYaw: 50,
                panSpeed: 20,
                panWidth: 30,
                buildingHeight: 6,
                buildingFill: 8,
                rgbSmoothing: 90,
                neonFlash: 15,
                streetLights: 22,
                colorIntensity: 25,
                colorSaturation: 12,
                lightIntensity: 30,
                fogDensity: 72,
            },
        },
        {
            name: 'Toxic Undercity',
            description: 'Chemical runoff district at sewer level — acid green reflections on wet concrete, hazmat signs flickering in the poisoned dark',
            controls: {
                cyberpunkMode: 'Neon',
                colorPalette: 'Toxic',
                speed: 5,
                zoom: 4,
                cameraPitch: 68,
                cameraRoll: 48,
                cameraYaw: 55,
                panSpeed: 40,
                panWidth: 55,
                buildingHeight: 3,
                buildingFill: 42,
                rgbSmoothing: 35,
                neonFlash: 75,
                streetLights: 88,
                colorIntensity: 70,
                colorSaturation: 80,
                lightIntensity: 60,
                fogDensity: 58,
            },
        },
        {
            name: 'Synthwave Flyover',
            description: 'Retro-future cruise above a neon grid city — magenta sunset bleeding into chrome skyline, synth bass you can feel in your teeth',
            controls: {
                cyberpunkMode: 'Neon',
                colorPalette: 'Synthwave',
                speed: 6,
                zoom: 6,
                cameraPitch: 42,
                cameraRoll: 52,
                cameraYaw: 50,
                panSpeed: 50,
                panWidth: 60,
                buildingHeight: 7,
                buildingFill: 28,
                rgbSmoothing: 55,
                neonFlash: 65,
                streetLights: 60,
                colorIntensity: 85,
                colorSaturation: 90,
                lightIntensity: 72,
                fogDensity: 40,
            },
        },
    ],
})
