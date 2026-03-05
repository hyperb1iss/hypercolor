export type { EffectConfig } from './base-effect'
export { BaseEffect } from './base-effect'
export type { CanvasEffectConfig } from './canvas-effect'
export { CanvasEffect } from './canvas-effect'
export type { UniformValue, WebGLEffectConfig } from './webgl-effect'
export { WebGLEffect } from './webgl-effect'

// ── New declarative API ──────────────────────────────────────────────────
export { effect } from './effect-fn'
export type { EffectFnOptions, ShaderContext } from './effect-fn'
export { canvas } from './canvas-fn'
export type { CanvasFnOptions, DrawFn, FactoryFn } from './canvas-fn'
