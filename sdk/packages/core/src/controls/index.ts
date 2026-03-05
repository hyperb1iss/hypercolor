export type {
    BaseControls,
    BooleanControlDefinition,
    ColorControlDefinition,
    ComboboxControlDefinition,
    ControlDefinition,
    ControlDefinitionType,
    ControlValues,
    HueControlDefinition,
    NumberControlDefinition,
    TextFieldControlDefinition,
} from './definitions'

export type {
    BooleanControlOptions,
    ColorControlOptions,
    ComboboxControlOptions,
    ControlDecoratorOptions,
    EffectOptions,
    HueControlOptions,
    NumberControlOptions,
    TextFieldControlOptions,
} from './decorators'

export {
    BooleanControl,
    ColorControl,
    ComboboxControl,
    Effect,
    extractControlsFromClass,
    extractEffectMetadata,
    getControlForProperty,
    HueControl,
    METADATA_KEYS,
    NumberControl,
    TextFieldControl,
} from './decorators'

export {
    boolToInt,
    comboboxValueToIndex,
    getAllControls,
    getControlValue,
    normalizePercentage,
    normalizeSpeed,
} from './helpers'

// ── New declarative control API ──────────────────────────────────────────
export { num, combo, toggle, color, hue, text, isControlSpec } from './specs'
export type { ControlSpec, ControlTypeName, NormalizeHint } from './specs'
export { inferControl } from './infer'
export type { ControlMap, ControlMapValue, ControlShorthand } from './infer'
export { deriveLabel, deriveUniformName } from './names'
