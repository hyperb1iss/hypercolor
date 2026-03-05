/**
 * Decorator-based control system for Hypercolor effects.
 * Uses reflect-metadata to attach control definitions to class properties.
 */

import 'reflect-metadata'
import { ControlDefinitionType } from './definitions'

/** Metadata keys for the reflection system. */
export const METADATA_KEYS = {
    controls: Symbol.for('hypercolor:controls'),
    effect: Symbol.for('hypercolor:effect'),
}

const propertyMetadataKey = (propertyName: string) => Symbol.for(`hypercolor:control:${propertyName}`)

// ── Decorator Option Types ──────────────────────────────────────────────

export interface ControlDecoratorOptions {
    label: string
    tooltip?: string
}

export interface NumberControlOptions extends ControlDecoratorOptions {
    min: number
    max: number
    default: number
    step?: number
}

export interface BooleanControlOptions extends ControlDecoratorOptions {
    default: boolean
}

export interface ComboboxControlOptions extends ControlDecoratorOptions {
    values: string[]
    default: string
}

export interface HueControlOptions extends ControlDecoratorOptions {
    min: number
    max: number
    default: number
}

export interface ColorControlOptions extends ControlDecoratorOptions {
    default: string
}

export interface TextFieldControlOptions extends ControlDecoratorOptions {
    default: string
}

// ── Decorator Factory ───────────────────────────────────────────────────

function createControlDecorator<T extends ControlDecoratorOptions>(
    createDefinition: (propertyKey: string, options: T) => ControlDefinitionType,
) {
    return (options: T): PropertyDecorator =>
        (target: object, propertyKey: string | symbol) => {
            if (typeof propertyKey !== 'string') {
                throw new Error('Control decorators can only be used on string properties')
            }

            const targetConstructor = target.constructor
            if (!Reflect.hasMetadata(METADATA_KEYS.controls, targetConstructor)) {
                Reflect.defineMetadata(METADATA_KEYS.controls, [], targetConstructor)
            }
            const controlsMetadata = Reflect.getMetadata(METADATA_KEYS.controls, targetConstructor)

            const controlDefinition = createDefinition(propertyKey, options)
            controlsMetadata.push(controlDefinition)

            Reflect.defineMetadata(propertyMetadataKey(propertyKey), controlDefinition, targetConstructor)
        }
}

// ── Control Decorators ──────────────────────────────────────────────────

/** Number slider control. */
export const NumberControl = createControlDecorator<NumberControlOptions>((propertyKey, options) => ({
    default: options.default,
    id: propertyKey,
    label: options.label,
    max: options.max,
    min: options.min,
    step: options.step,
    tooltip: options.tooltip,
    type: 'number',
}))

/** Boolean toggle control. */
export const BooleanControl = createControlDecorator<BooleanControlOptions>((propertyKey, options) => ({
    default: options.default,
    id: propertyKey,
    label: options.label,
    tooltip: options.tooltip,
    type: 'boolean',
}))

/** Dropdown combobox control. */
export const ComboboxControl = createControlDecorator<ComboboxControlOptions>((propertyKey, options) => ({
    default: options.default,
    id: propertyKey,
    label: options.label,
    tooltip: options.tooltip,
    type: 'combobox',
    values: options.values,
}))

/** Hue wheel control. */
export const HueControl = createControlDecorator<HueControlOptions>((propertyKey, options) => ({
    default: options.default,
    id: propertyKey,
    label: options.label,
    max: options.max,
    min: options.min,
    tooltip: options.tooltip,
    type: 'hue',
}))

/** Color picker control. */
export const ColorControl = createControlDecorator<ColorControlOptions>((propertyKey, options) => ({
    default: options.default,
    id: propertyKey,
    label: options.label,
    tooltip: options.tooltip,
    type: 'color',
}))

/** Text input control. */
export const TextFieldControl = createControlDecorator<TextFieldControlOptions>((propertyKey, options) => ({
    default: options.default,
    id: propertyKey,
    label: options.label,
    tooltip: options.tooltip,
    type: 'textfield',
}))

// ── Effect Class Decorator ──────────────────────────────────────────────

export interface EffectOptions {
    name: string
    description: string
    author: string
    audioReactive?: boolean
}

/** Class decorator to mark a class as a Hypercolor effect. */
export function Effect(options: EffectOptions): ClassDecorator {
    return (target: { prototype: Record<string, unknown> }) => {
        Reflect.defineMetadata(METADATA_KEYS.effect, options, target.prototype)
        target.prototype.effectMetadata = options
    }
}

// ── Extraction Utilities ────────────────────────────────────────────────

/** Extract control definitions from a decorated class. */
export function extractControlsFromClass(targetClass: unknown): ControlDefinitionType[] {
    const targetConstructor =
        typeof targetClass === 'function'
            ? (targetClass as { new (...args: unknown[]): unknown })
            : (targetClass as object).constructor

    if (Reflect.hasMetadata(METADATA_KEYS.controls, targetConstructor)) {
        return Reflect.getMetadata(METADATA_KEYS.controls, targetConstructor)
    }

    return []
}

/** Extract effect metadata from a decorated class. */
export function extractEffectMetadata(targetClass: unknown): EffectOptions {
    const defaultMetadata = { author: '', description: '', name: 'Unnamed Effect' }

    const prototype =
        typeof targetClass === 'function' ? targetClass.prototype : Object.getPrototypeOf(targetClass as object)

    if (Reflect.hasMetadata(METADATA_KEYS.effect, prototype)) {
        return Reflect.getMetadata(METADATA_KEYS.effect, prototype)
    }

    if (prototype && 'effectMetadata' in prototype) {
        return prototype.effectMetadata as EffectOptions
    }

    return defaultMetadata
}

/** Get a specific control definition for a property. */
export function getControlForProperty(targetClass: unknown, propertyName: string): ControlDefinitionType | undefined {
    return Reflect.getMetadata(propertyMetadataKey(propertyName), targetClass as object)
}
