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

export {
    boolToInt,
    comboboxValueToIndex,
    getAllControls,
    getControlValue,
    normalizePercentage,
    normalizeSpeed,
} from './helpers'

// ── Declarative control API ──────────────────────────────────────────
export { num, combo, toggle, color, hue, text, isControlSpec } from './specs'
export type { ControlSpec, ControlTypeName, NormalizeHint } from './specs'
export { inferControl } from './infer'
export type { ControlMap, ControlMapValue, ControlShorthand } from './infer'
export { deriveLabel, deriveUniformName } from './names'
