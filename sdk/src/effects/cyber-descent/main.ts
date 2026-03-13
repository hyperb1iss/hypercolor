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

const COLOR_PALETTES = [
    'Akira Red',
    'Blade Runner',
    'Classic Cyber',
    'Ice',
    'Matrix',
    'Noir',
    'Synthwave',
    'Toxic',
] as const

export default effect(
    'Cyber Descent',
    shader,
    {
        buildingFill: num('Building Fill', [0, 100], 20, { group: 'City', tooltip: 'Surface glow density for RGB' }),

        buildingHeight: num('Building Height', [1, 10], 5, { group: 'City' }),
        cameraPitch: num('Camera Pitch', [0, 100], 50, {
            group: 'Camera',
            tooltip: 'Vertical look angle (50 = level)',
        }),
        cameraRoll: num('Camera Roll', [0, 100], 50, { group: 'Camera', tooltip: 'Banking angle (50 = level)' }),
        cameraYaw: num('Camera Yaw', [0, 100], 50, { group: 'Camera', tooltip: 'Horizontal turn (50 = forward)' }),

        colorIntensity: num('Color Intensity', [0, 100], 50, { group: 'Color' }),
        colorPalette: combo('Color Palette', COLOR_PALETTES, { group: 'Scene' }),
        colorSaturation: num('Color Saturation', [0, 100], 50, { group: 'Color' }),
        cyberpunkMode: combo('City Style', CITY_STYLES, { group: 'Scene' }),
        fogDensity: num('Fog Density', [0, 100], 50, { group: 'Color' }),
        lightIntensity: num('Light Intensity', [0, 100], 50, { group: 'Color' }),
        neonFlash: num('Neon Flash', [0, 100], 50, { group: 'City' }),
        panSpeed: num('Pan Speed', [0, 100], 30, { group: 'Camera', tooltip: 'Horizontal weaving speed' }),
        panWidth: num('Pan Width', [0, 100], 40, { group: 'Camera', tooltip: 'How wide the camera weaves' }),
        rgbSmoothing: num('RGB Smoothing', [0, 100], 60, {
            group: 'City',
            tooltip: 'Softens thin building detail for LED layouts',
        }),

        speed: num('Flight Speed', [1, 10], 5, { group: 'Camera' }),
        streetLights: num('Street Lights', [0, 100], 50, { group: 'City' }),
        zoom: num('Camera Zoom', [1, 10], 5, { group: 'Camera' }),
    },
    {
        description: 'Cyberpunk city flythrough with raymarched buildings, neon signs, and horizontal camera panning',
        presets: [
            {
                controls: {
                    colorPalette: 'Akira Red',
                    cyberpunkMode: 'Fast Descent',
                    buildingFill: 35,
                    buildingHeight: 8,
                    cameraPitch: 62,
                    cameraRoll: 54,
                    cameraYaw: 50,
                    colorIntensity: 72,
                    colorSaturation: 78,
                    fogDensity: 30,
                    lightIntensity: 60,
                    neonFlash: 70,
                    panSpeed: 65,
                    panWidth: 70,
                    rgbSmoothing: 45,
                    speed: 9,
                    streetLights: 70,
                    zoom: 3,
                },
                description:
                    'Bullet-train POV through the Akira sprawl — red neon screaming past at terminal velocity, buildings a smear of crimson and chrome',
                name: 'Neo-Tokyo Express',
            },
            {
                controls: {
                    buildingFill: 12,
                    buildingHeight: 10,
                    cameraPitch: 35,
                    cameraRoll: 50,
                    cameraYaw: 48,
                    colorIntensity: 55,
                    colorPalette: 'Blade Runner',
                    colorSaturation: 40,
                    cyberpunkMode: 'Standard',
                    fogDensity: 85,
                    lightIntensity: 38,
                    neonFlash: 28,
                    panSpeed: 15,
                    panWidth: 22,
                    rgbSmoothing: 80,
                    speed: 3,
                    streetLights: 35,
                    zoom: 7,
                },
                description:
                    'Slow vertical ascent through perpetual rain — Blade Runner amber cutting through fog, pyramidal towers materializing from the murk',
                name: 'Tyrell Tower Approach',
            },
            {
                controls: {
                    buildingFill: 15,
                    buildingHeight: 6,
                    cameraPitch: 50,
                    cameraRoll: 50,
                    cameraYaw: 50,
                    colorIntensity: 48,
                    colorPalette: 'Noir',
                    colorSaturation: 32,
                    cyberpunkMode: 'Standard',
                    fogDensity: 55,
                    lightIntensity: 50,
                    neonFlash: 30,
                    panSpeed: 20,
                    panWidth: 30,
                    rgbSmoothing: 90,
                    speed: 4,
                    streetLights: 42,
                    zoom: 5,
                },
                description:
                    'The sky above the port — colorless noir cityscape dissolving into signal noise, monochrome towers fading at the edge of perception',
                name: 'Dead Channel Static',
            },
            {
                controls: {
                    buildingFill: 42,
                    buildingHeight: 3,
                    cameraPitch: 68,
                    cameraRoll: 48,
                    cameraYaw: 55,
                    colorIntensity: 70,
                    colorPalette: 'Toxic',
                    colorSaturation: 80,
                    cyberpunkMode: 'Neon',
                    fogDensity: 58,
                    lightIntensity: 60,
                    neonFlash: 75,
                    panSpeed: 40,
                    panWidth: 55,
                    rgbSmoothing: 35,
                    speed: 5,
                    streetLights: 88,
                    zoom: 4,
                },
                description:
                    'Chemical runoff district at sewer level — acid green reflections on wet concrete, hazmat signs flickering in the poisoned dark',
                name: 'Toxic Undercity',
            },
            {
                controls: {
                    buildingFill: 28,
                    buildingHeight: 7,
                    cameraPitch: 42,
                    cameraRoll: 52,
                    cameraYaw: 50,
                    colorIntensity: 58,
                    colorPalette: 'Synthwave',
                    colorSaturation: 62,
                    cyberpunkMode: 'Neon',
                    fogDensity: 40,
                    lightIntensity: 52,
                    neonFlash: 65,
                    panSpeed: 50,
                    panWidth: 60,
                    rgbSmoothing: 55,
                    speed: 6,
                    streetLights: 60,
                    zoom: 6,
                },
                description:
                    'Retro-future cruise above a neon grid city — magenta sunset bleeding into chrome skyline, synth bass you can feel in your teeth',
                name: 'Synthwave Flyover',
            },
        ],
    },
)
