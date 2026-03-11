#version 300 es
// Cyber Descent — Cyberpunk city flythrough
// Adapted from Shadertoy: https://www.shadertoy.com/view/wdfGW4
// Ported to Hypercolor SDK with horizontal panning
precision highp float;

out vec4 fragColor;

// ── Standard uniforms ───────────────────────────────────────────
uniform float iTime;
uniform vec2 iResolution;

// ── Control uniforms (raw values from UI sliders) ───────────────
uniform float iSpeed;            // 1–10
uniform float iZoom;             // 1–10
uniform int iCyberpunkMode;      // combo index (0–2)
uniform int iColorPalette;       // combo index (0–7)
uniform float iCameraPitch;      // 0–100, 50 = level
uniform float iCameraRoll;       // 0–100, 50 = level
uniform float iCameraYaw;        // 0–100, 50 = forward
uniform float iBuildingHeight;   // 1–10
uniform float iBuildingFill;     // 0–100
uniform float iRgbSmoothing;     // 0–100
uniform float iNeonFlash;        // 0–100
uniform float iStreetLights;     // 0–100
uniform float iColorIntensity;   // 0–100
uniform float iColorSaturation;  // 0–100
uniform float iLightIntensity;   // 0–100
uniform float iFogDensity;       // 0–100
uniform float iPanSpeed;         // 0–100
uniform float iPanWidth;         // 0–100

// ── Normalized control values (set in main before any function calls) ──
float cSpeed, cZoom;
float cPitch, cRoll, cYaw;
float cBuildingHeight, cBuildingFill;
float cRgbSmooth;
float cNeonFlash, cStreetLights;
float cColorInt, cColorSat, cLightInt, cFogDens;
float cPanSpeed, cPanWidth;

// ── Constants ───────────────────────────────────────────────────
const float tau = 6.283185;

// ── Camera and animation (set by initializeSettings) ────────────
vec3 cameraDir;
float cameraDist;
float speed;
float zoom;

// ── Color palette (set by initializeColorPalette) ───────────────
vec3 windowColorA;
vec3 windowColorB;
vec3 fogColor;
vec3 lightColorA;
vec3 lightColorB;
vec3 signColorA;
vec3 signColorB;

// ── Fog and lights ──────────────────────────────────────────────
float fogOffset;
float fogDensity;
float lightHeight;
float lightSpeed;

// ── Control normalization ───────────────────────────────────────
void normalizeControls() {
    cSpeed         = iSpeed / 5.0;                    // 0.2–2.0
    cZoom          = iZoom / 5.0;                     // 0.2–2.0
    cPitch         = (iCameraPitch - 50.0) / 50.0;   // -1 to +1
    cRoll          = (iCameraRoll - 50.0) / 50.0;    // -1 to +1
    cYaw           = (iCameraYaw - 50.0) / 50.0;     // -1 to +1
    cBuildingHeight = iBuildingHeight / 5.0;          // 0.2–2.0
    cBuildingFill  = iBuildingFill / 100.0;           // 0–1
    cRgbSmooth     = iRgbSmoothing / 100.0;           // 0–1
    cNeonFlash     = iNeonFlash / 50.0;               // 0–2
    cStreetLights  = iStreetLights / 50.0;            // 0–2
    cColorInt      = iColorIntensity / 50.0;          // 0–2
    cColorSat      = iColorSaturation / 50.0;         // 0–2
    cLightInt      = iLightIntensity / 50.0;          // 0–2
    cFogDens       = iFogDensity / 50.0;              // 0–2
    cPanSpeed      = iPanSpeed / 50.0;                // 0–2
    cPanWidth      = iPanWidth / 50.0;                // 0–2
}

// ── Color palette initialization ────────────────────────────────
void initializeColorPalette() {
    float neonBoost = 1.0;
    float satBoost = 1.0;
    float darkBoost = 1.0;

    if (iCyberpunkMode == 0) {
        darkBoost = 0.7;
        satBoost = 1.2;
    } else if (iCyberpunkMode == 1) {
        neonBoost = 1.4;
        satBoost = 1.3;
    }

    if (iColorPalette == 1) {
        // Blade Runner
        windowColorA = vec3(1.8, 1.0, 0.3);
        windowColorB = vec3(1.0, 0.6, 0.2);
        fogColor     = vec3(0.08, 0.12, 0.18);
        lightColorA  = vec3(1.2, 0.5, 0.2) * cLightInt;
        lightColorB  = vec3(0.3, 0.8, 1.0) * cLightInt;
        signColorA   = vec3(2.0, 0.3, 0.8);
        signColorB   = vec3(1.5, 1.8, 0.2);
    } else if (iColorPalette == 6) {
        // Synthwave
        windowColorA = vec3(0.2, 1.8, 2.0);
        windowColorB = vec3(2.0, 0.2, 1.2);
        fogColor     = vec3(0.12, 0.02, 0.18);
        lightColorA  = vec3(1.0, 0.4, 1.5) * cLightInt;
        lightColorB  = vec3(0.2, 1.2, 1.5) * cLightInt;
        signColorA   = vec3(2.5, 1.5, 0.0);
        signColorB   = vec3(0.0, 2.5, 2.0);
    } else if (iColorPalette == 4) {
        // Matrix
        windowColorA = vec3(0.0, 2.0, 0.4);
        windowColorB = vec3(0.2, 1.2, 0.3);
        fogColor     = vec3(0.0, 0.02, 0.01);
        lightColorA  = vec3(0.3, 1.5, 0.5) * cLightInt;
        lightColorB  = vec3(1.5, 1.5, 1.5) * cLightInt;
        signColorA   = vec3(0.0, 2.5, 0.6);
        signColorB   = vec3(2.0, 2.0, 2.0);
    } else if (iColorPalette == 0) {
        // Akira Red
        windowColorA = vec3(2.0, 0.3, 0.1);
        windowColorB = vec3(1.5, 0.8, 0.2);
        fogColor     = vec3(0.1, 0.05, 0.08);
        lightColorA  = vec3(1.5, 0.4, 0.2) * cLightInt;
        lightColorB  = vec3(0.2, 0.8, 1.2) * cLightInt;
        signColorA   = vec3(0.0, 1.8, 2.0);
        signColorB   = vec3(2.5, 2.0, 0.0);
    } else if (iColorPalette == 3) {
        // Ice
        windowColorA = vec3(0.8, 1.8, 2.2);
        windowColorB = vec3(1.8, 1.9, 2.0);
        fogColor     = vec3(0.05, 0.1, 0.18);
        lightColorA  = vec3(0.6, 0.9, 1.2) * cLightInt;
        lightColorB  = vec3(1.2, 0.6, 0.8) * cLightInt;
        signColorA   = vec3(2.0, 0.5, 1.0);
        signColorB   = vec3(0.5, 2.0, 2.5);
    } else if (iColorPalette == 7) {
        // Toxic
        windowColorA = vec3(0.6, 2.0, 0.2);
        windowColorB = vec3(1.8, 1.8, 0.0);
        fogColor     = vec3(0.08, 0.06, 0.02);
        lightColorA  = vec3(0.5, 1.5, 0.2) * cLightInt;
        lightColorB  = vec3(1.5, 0.8, 0.2) * cLightInt;
        signColorA   = vec3(2.0, 0.5, 0.0);
        signColorB   = vec3(0.2, 2.5, 0.4);
    } else if (iColorPalette == 5) {
        // Noir
        windowColorA = vec3(0.3, 0.4, 0.8);
        windowColorB = vec3(0.5, 0.6, 0.7);
        fogColor     = vec3(0.02, 0.02, 0.04);
        lightColorA  = vec3(0.4, 0.45, 0.6) * cLightInt;
        lightColorB  = vec3(1.2, 0.3, 0.2) * cLightInt;
        signColorA   = vec3(1.8, 0.2, 0.2);
        signColorB   = vec3(0.8, 0.9, 1.2);
    } else {
        // Classic Cyber (default)
        windowColorA = vec3(0.0, 0.5, 2.0);
        windowColorB = vec3(0.5, 1.8, 2.0);
        fogColor     = vec3(0.2, 0.0, 0.25);
        lightColorA  = vec3(1.0, 0.5, 0.2) * cLightInt;
        lightColorB  = vec3(0.8, 0.6, 0.4) * cLightInt;
        signColorA   = vec3(2.0, 0.2, 1.0);
        signColorB   = vec3(0.2, 2.5, 2.0);
    }

    // Mode-specific boosts
    windowColorA *= neonBoost;
    windowColorB *= neonBoost;
    signColorA   *= satBoost;
    signColorB   *= satBoost;
    lightColorA  *= neonBoost;
    lightColorB  *= neonBoost;
    fogColor     *= darkBoost;

    // Mode-specific fog tints
    if (iCyberpunkMode == 0) {
        fogColor = mix(fogColor, vec3(0.05, 0.08, 0.15), 0.3);
    } else if (iCyberpunkMode == 1) {
        fogColor = mix(fogColor, vec3(0.18, 0.0, 0.25), 0.5);
    }
}

// ── Camera controls ─────────────────────────────────────────────
vec3 applyCameraControls(vec3 baseDir) {
    // Yaw — turn left/right
    float yawAngle = cYaw * 0.8;
    float cosYaw = cos(yawAngle);
    float sinYaw = sin(yawAngle);
    vec3 yawed = vec3(
        baseDir.x * cosYaw - baseDir.y * sinYaw,
        baseDir.x * sinYaw + baseDir.y * cosYaw,
        baseDir.z
    );

    // Pitch — tilt up/down
    float pitchAngle = cPitch * 0.6;
    float cosPitch = cos(pitchAngle);
    float sinPitch = sin(pitchAngle);
    vec3 pitched = vec3(
        yawed.x,
        yawed.y * cosPitch - yawed.z * sinPitch,
        yawed.y * sinPitch + yawed.z * cosPitch
    );

    return normalize(pitched);
}

// ── Settings initialization ─────────────────────────────────────
void initializeSettings() {
    vec3 baseDir;

    if (iCyberpunkMode == 0) {
        // Fast Descent — steeper, faster dive
        baseDir    = vec3(-2.0, -1.0, -4.0);
        cameraDist = 5.0;
        speed      = 3.0 * cSpeed;
        zoom       = 2.5 * cZoom;
        fogOffset  = 2.5;
        fogDensity = 0.6 * cFogDens;
        lightHeight = 0.5;
        lightSpeed = 0.2 * cSpeed;
    } else if (iCyberpunkMode == 1) {
        // Neon — medium angle, glowy atmosphere
        baseDir    = vec3(-1.5, -1.0, -3.0);
        cameraDist = 7.0;
        speed      = 2.0 * cSpeed;
        zoom       = 3.0 * cZoom;
        fogOffset  = 5.0;
        fogDensity = 0.65 * cFogDens;
        lightHeight = 0.3;
        lightSpeed = 0.18 * cSpeed;
    } else {
        // Standard — balanced view
        baseDir    = vec3(-2.0, -1.0, -2.0);
        cameraDist = 9.0;
        speed      = 1.0 * cSpeed;
        zoom       = 3.5 * cZoom;
        fogOffset  = 7.0;
        fogDensity = 0.7 * cFogDens;
        lightHeight = 0.0;
        lightSpeed = 0.15 * cSpeed;
    }

    cameraDir = applyCameraControls(baseDir);
    initializeColorPalette();
}

// ── Hash functions ──────────────────────────────────────────────
float hash1(float p) {
    vec3 p3 = fract(p * vec3(5.3983, 5.4427, 6.9371));
    p3 += dot(p3, p3.yzx + 19.19);
    return fract((p3.x + p3.y) * p3.z);
}

float hash1(vec2 p2) {
    p2 = fract(p2 * vec2(5.3983, 5.4427));
    p2 += dot(p2.yx, p2.xy + vec2(21.5351, 14.3137));
    return fract(p2.x * p2.y * 95.4337);
}

float hash1(vec2 p2, float p) {
    vec3 p3 = fract(vec3(5.3983 * p2.x, 5.4427 * p2.y, 6.9371 * p));
    p3 += dot(p3, p3.yzx + 19.19);
    return fract((p3.x + p3.y) * p3.z);
}

vec2 hash2(vec2 p2, float p) {
    vec3 p3 = fract(vec3(5.3983 * p2.x, 5.4427 * p2.y, 6.9371 * p));
    p3 += dot(p3, p3.yzx + 19.19);
    return fract((p3.xx + p3.yz) * p3.zy);
}

vec3 hash3(vec2 p2) {
    vec3 p3 = fract(vec3(p2.xyx) * vec3(5.3983, 5.4427, 6.9371));
    p3 += dot(p3, p3.yxz + 19.19);
    return fract((p3.xxy + p3.yzz) * p3.zyx);
}

vec4 hash4(vec2 p2) {
    vec4 p4 = fract(p2.xyxy * vec4(5.3983, 5.4427, 6.9371, 7.1283));
    p4 += dot(p4, p4.yxwz + 19.19);
    return fract((p4.xxxy + p4.yyzz + p4.zwww) * p4.wzyx);
}

float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash1(i + vec2(0.0, 0.0)), hash1(i + vec2(1.0, 0.0)), u.x),
        mix(hash1(i + vec2(0.0, 1.0)), hash1(i + vec2(1.0, 1.0)), u.x),
        u.y
    );
}

float repeatedBand(float coord, float period, float width, float softness) {
    float scaled = coord / max(period, 0.0001);
    float stripe = abs(fract(scaled) - 0.5);
    float aa = max(fwidth(scaled), 0.015) * (1.0 + 3.5 * softness);
    return 1.0 - smoothstep(width, width + aa, stripe);
}

// ── Raymarching ─────────────────────────────────────────────────
vec4 castRay(vec3 eye, vec3 ray, vec2 center) {
    vec2 block = floor(eye.xy);
    vec3 ri = 1.0 / ray;
    vec3 rs = sign(ray);
    vec3 side = 0.5 + 0.5 * rs;
    vec2 ris = ri.xy * rs.xy;
    vec2 dis = (block - eye.xy + 0.5 + rs.xy * 0.5) * ri.xy;

    for (int i = 0; i < 16; ++i) {
        float d = dot(block - center, cameraDir.xy);
        float heightScale = 0.4 + cBuildingHeight * 1.3;
        float height = (3.0 * hash1(block) - 1.0 + 1.5 * d - 0.1 * d * d) * heightScale;

        vec2 lo0 = vec2(block);
        vec2 loX = vec2(0.45, 0.45);
        vec2 hi0 = vec2(block + 0.55);
        vec2 hiX = vec2(0.45, 0.45);

        float dist = 500.0;
        float face = 0.0;

        {
            vec4 signHash = hash4(block);
            vec2 center = vec2(0.2, -0.4) + vec2(0.6, -0.8) * signHash.xy;
            float width = 0.06 + 0.1 * signHash.w;

            vec3 lo = vec3(center.x - width, 0.55, -100.0);
            vec3 hi = vec3(center.x + width, 0.99, center.y + width + height);

            float s = step(0.5, signHash.z);
            lo = vec3(block, 0.0) + mix(lo, lo.yxz, s);
            hi = vec3(block, 0.0) + mix(hi, hi.yxz, s);

            vec3 wall = mix(hi, lo, side);
            vec3 t = (wall - eye) * ri;

            vec3 dim = step(t.zxy, t) * step(t.yzx, t);
            float maxT = dot(dim, t);
            float maxFace = dim.x - dim.y;

            vec3 p = eye + maxT * ray;
            dim += step(lo, p) * step(p, hi);

            if (dim.x * dim.y * dim.z > 0.5) {
                dist = maxT;
                face = maxFace;
            }
        }

        for (int j = 0; j < 5; ++j) {
            float top = height - 0.4 * float(j);
            vec3 lo = vec3(lo0 + loX * hash2(block, float(j)), -100.0);
            vec3 hi = vec3(hi0 + hiX * hash2(block, float(j) + 0.5), top);

            vec3 wall = mix(hi, lo, side);
            vec3 t = (wall - eye) * ri;

            vec3 dim = step(t.zxy, t) * step(t.yzx, t);
            float maxT = dot(dim, t);
            float maxFace = dim.x - dim.y;

            vec3 p = eye + maxT * ray;
            dim += step(lo, p) * step(p, hi);

            if (dim.x * dim.y * dim.z > 0.5 && maxT < dist) {
                dist = maxT;
                face = maxFace;
            }
        }

        if (dist < 400.0) {
            return vec4(dist, height, face, 1.0);
        }

        float t = eye.z * ri.z;
        vec3 p = eye - t * ray;
        vec2 g = p.xy - block;

        vec2 dim = step(dis.xy, dis.yx);
        dis += dim * ris;
        block += dim * rs.xy;
    }

    return vec4(100.0, 0.0, 0.0, 1.0);
}

// ── Building windows ────────────────────────────────────────────
vec3 window(float z, vec2 pos, vec2 id) {
    float windowSize = (0.03 + 0.12 * hash1(id + 0.1)) * mix(1.0, 2.4, cRgbSmooth);
    float baseProb = 0.3 + 0.8 * hash1(id + 0.2);
    float windowProb = baseProb - cBuildingFill * 0.4;
    float depth = z / max(windowSize, 0.0001);
    float level = floor(depth);
    vec3 colorA = mix(windowColorA, windowColorB, hash3(id));
    vec3 colorB = mix(windowColorA, windowColorB, hash3(id + 0.1));
    vec3 color = mix(colorA, colorB, hash1(id, level));
    float facadeNoiseFreq = mix(20.0, 7.0, cRgbSmooth);
    color *= 0.45 + 0.55 * smoothstep(
        0.15,
        0.55,
        noise(facadeNoiseFreq * pos + 60.0 * hash1(level))
    );

    float windowMask = smoothstep(
        windowProb - 0.2,
        windowProb + 0.2,
        hash1(id, level + 0.1)
    );

    // Ambient surface glow
    vec3 surfaceGlow = mix(windowColorA, fogColor, 0.6) * cBuildingFill * mix(0.3, 0.45, cRgbSmooth);

    float bandWidth = mix(0.18, 0.34, cRgbSmooth);
    float windowBand = repeatedBand(z, windowSize, bandWidth, cRgbSmooth);
    vec3 windowColor = color * windowMask * windowBand;
    return windowColor + surfaceGlow;
}

// ── Flying lights ───────────────────────────────────────────────
vec3 addLight(vec3 eye, vec3 ray, float res, float time, float height) {
    vec2 q = eye.xy + (height - eye.z) / ray.z * ray.xy;

    float row = floor(q.x + 0.5);
    time += hash1(row);
    float col = floor(0.125 * q.y - time);

    float pos = 0.4 + 0.4 * cos(time + tau * hash1(vec2(row, col)));
    vec3 lightPos = vec3(row, 8.0 * (col + time + pos), height);
    vec3 lightDir = vec3(0.0, 1.0, 0.0);

    vec3 w = eye - lightPos;
    float a = dot(ray, ray);
    float b = dot(ray, lightDir);
    float c = dot(lightDir, lightDir);
    float d = dot(ray, w);
    float e = dot(lightDir, w);
    float D = a * c - b * b;
    float s = (b * e - c * d) / D;
    float t = (a * e - b * d) / D;

    t = max(t, 0.0);
    float dist = distance(eye + s * ray, lightPos + t * lightDir);

    float mask = smoothstep(res + 0.1, res, s);
    float light = min(
        1.0 / pow(200.0 * dist * dist / t + 20.0 * t * t, 0.8),
        2.0
    );
    float fog = exp(-fogDensity * max(s - fogOffset, 0.0));
    vec3 color = mix(lightColorA, lightColorB, hash3(vec2(row, col)));
    return mask * light * fog * color;
}

// ── Neon signs ──────────────────────────────────────────────────
vec3 addSign(vec3 color, vec3 pos, float side, vec2 id) {
    vec4 signHash = hash4(id);
    float s = step(0.5, signHash.z);
    if ((s - 0.5) * side < 0.1) return color;

    vec2 center = vec2(0.2, -0.4) + vec2(0.6, -0.8) * signHash.xy;
    vec2 p = mix(pos.xz, pos.yz, s);
    float halfWidth = (0.04 + 0.06 * signHash.w) * mix(1.0, 1.75, cRgbSmooth);

    float charCount = floor(1.0 + 8.0 * hash1(id + 0.5));
    if (center.y - p.y > 2.0 * halfWidth * (charCount + 1.0)) {
        center.y -= 2.0 * halfWidth * (charCount + 1.5 + 5.0 * hash1(id + 0.6));
        charCount = floor(2.0 + 12.0 * hash1(id + 0.7));
        id += 0.05;
    }
    charCount = max(1.0, floor(mix(charCount, max(2.0, charCount * 0.65), cRgbSmooth)));

    vec3 signColor = mix(signColorA, signColorB, hash3(id + 0.5));
    vec3 outlineColor = mix(signColorA, signColorB, hash3(id + 0.6));

    // Flash intensity: 0 = static, 2 = rave
    float flashBase = 6.0 - 24.0 * hash1(id + 0.8);
    flashBase *= step(3.0, flashBase);
    float flashSpeed = flashBase * (0.5 + cNeonFlash * 1.5);
    float flash = mix(
        1.0,
        smoothstep(0.1, 0.5, 0.5 + 0.5 * cos(flashSpeed * iTime)),
        min(cNeonFlash, 1.0)
    );

    vec2 halfSize = vec2(halfWidth, halfWidth * charCount);
    center.y -= halfSize.y;
    float outline = length(max(abs(p - center) - halfSize, 0.0)) / halfWidth;
    float outlineAA = max(fwidth(outline), 0.02) * (1.0 + 2.5 * cRgbSmooth);
    color *= smoothstep(0.1, 0.4 + outlineAA, outline);

    vec2 charPos = 0.5 * (p - center + halfSize) / halfWidth;
    vec2 charId = id + 0.05 + 0.1 * floor(charPos);
    float blinkSeed = hash1(charId);
    float randomBlink = step(0.93, blinkSeed);
    randomBlink = 1.0 - randomBlink * step(0.96, hash1(charId, iTime));
    float softBlink = 0.72 + 0.28 * sin(
        tau * hash1(charId + 0.3) + iTime * (1.5 + 2.5 * hash1(charId + 0.4))
    );
    float flicker = mix(randomBlink, softBlink, cRgbSmooth);

    float char_ = -3.5 + 8.0 * noise(id + 6.0 * charPos);
    charPos = fract(charPos);
    float charAA = max(max(fwidth(charPos.x), fwidth(charPos.y)), 0.02) * (1.0 + 2.0 * cRgbSmooth);
    float charMaskX = smoothstep(0.0, 0.4 + charAA, charPos.x)
                    * (1.0 - smoothstep(0.6 - charAA, 1.0, charPos.x));
    float charMaskY = smoothstep(0.0, 0.4 + charAA, charPos.y)
                    * (1.0 - smoothstep(0.6 - charAA, 1.0, charPos.y));
    char_ *= charMaskX * charMaskY;
    color = mix(
        color,
        signColor,
        flash * flicker * (1.0 - smoothstep(0.01, 0.01 + outlineAA, outline)) * clamp(char_, 0.0, 1.0)
    );

    outline = smoothstep(0.0, 0.2 + outlineAA, outline)
            * (1.0 - smoothstep(0.3 - outlineAA, 0.5 + outlineAA, outline));
    return mix(color, outlineColor, flash * outline);
}

vec3 renderScene(vec2 fragCoord) {
    // Camera position along flight path
    vec2 center = -speed * iTime * cameraDir.xy;

    // ── Horizontal panning — weave through the city ─────────────
    if (cPanWidth > 0.001) {
        vec2 panPerp = normalize(vec2(-cameraDir.y, cameraDir.x));
        float panPhase = iTime * cPanSpeed * 0.5;
        float panOffset = sin(panPhase) * cPanWidth * 2.0
                        + sin(panPhase * 1.73 + 0.7) * cPanWidth * 0.8;
        center += panPerp * panOffset;
    }

    vec3 eye = vec3(center, 0.0) - cameraDist * cameraDir;

    vec3 forward = normalize(cameraDir);
    vec3 right = normalize(cross(forward, vec3(0.0, 0.0, 1.0)));
    vec3 up = cross(right, forward);

    // Camera roll (banking)
    float rollAngle = cRoll * 0.5;
    float cosRoll = cos(rollAngle);
    float sinRoll = sin(rollAngle);
    vec3 rolledRight = right * cosRoll + up * sinRoll;
    vec3 rolledUp    = up * cosRoll - right * sinRoll;

    vec2 xy = 2.0 * fragCoord - iResolution.xy;
    vec3 ray = normalize(
        xy.x * rolledRight + xy.y * rolledUp + zoom * forward * iResolution.y
    );

    vec4 res = castRay(eye, ray, center);
    vec3 p = eye + res.x * ray;

    vec2 block = floor(p.xy);
    vec3 color = window(p.z - res.y, p.xy, block);

    color = addSign(color, vec3(p.xy - block, p.z - res.y), res.z, block);

    // Edge glow at building boundaries
    float edgeFactor = 1.0 - abs(res.z);
    vec3 edgeGlow = mix(windowColorA, windowColorB, 0.5) * edgeFactor * cBuildingFill * 0.5;
    color = mix(edgeGlow, color, abs(res.z));

    // Atmospheric fog
    float fog = exp(-fogDensity * max(res.x - fogOffset, 0.0));
    color = mix(fogColor, color, fog);

    // Street / flying lights
    float time = lightSpeed * iTime;
    float lightDensity = cStreetLights;

    // Base light layers
    color += addLight(eye.xyz, ray.xyz, res.x, time, lightHeight - 0.6) * min(lightDensity * 2.0, 1.0);
    color += addLight(eye.yxz, ray.yxz, res.x, time, lightHeight - 0.4) * min(lightDensity * 2.0, 1.0);

    // Extra layers at higher density
    if (lightDensity > 0.5) {
        float extraIntensity = (lightDensity - 0.5) * 2.0;
        color += addLight(
            vec3(-eye.xy, eye.z),
            vec3(-ray.xy, ray.z),
            res.x, time, lightHeight - 0.2
        ) * extraIntensity;
        color += addLight(
            vec3(-eye.yx, eye.z),
            vec3(-ray.yx, ray.z),
            res.x, time, lightHeight
        ) * extraIntensity;
    }

    // Swarm lights at max density
    if (lightDensity > 1.5) {
        float swarmIntensity = (lightDensity - 1.5) * 2.0;
        color += addLight(eye.xyz, ray.xyz, res.x, time * 1.3, lightHeight + 0.3) * swarmIntensity * 0.7;
        color += addLight(eye.yxz, ray.yxz, res.x, time * 0.8, lightHeight - 0.8) * swarmIntensity * 0.7;
    }

    // B&W mode at very low saturation
    if (cColorSat < 0.1) {
        float c = clamp(dot(vec3(0.4, 0.3, 0.4), color), 0.0, 1.0);
        c = 1.0 - pow(1.0 - pow(c, 2.0), 4.0);
        color = vec3(c);
    }

    return color;
}

// ── Main ────────────────────────────────────────────────────────
void main() {
    // Normalize all UI controls to shader-internal ranges
    normalizeControls();

    // Initialize camera, fog, and color settings
    initializeSettings();

    // Apply color saturation and intensity
    windowColorA *= cColorSat * cColorInt;
    windowColorB *= cColorSat * cColorInt;
    signColorA   *= cColorSat * cColorInt;
    signColorB   *= cColorSat * cColorInt;

    vec3 color = renderScene(gl_FragCoord.xy);
    if (cRgbSmooth > 0.001) {
        vec2 aaOffset = vec2(0.42, 0.58) * cRgbSmooth;
        vec3 shifted = renderScene(gl_FragCoord.xy + aaOffset);
        color = mix(color, 0.5 * (color + shifted), 0.65 * cRgbSmooth);
    }

    fragColor = vec4(color, 1.0);
}
