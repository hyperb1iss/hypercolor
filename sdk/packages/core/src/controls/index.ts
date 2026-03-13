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
export type { ControlMap, ControlMapValue, ControlShorthand } from './infer'
export { inferControl } from './infer'
export { deriveLabel, deriveUniformName } from './names'
export type { ControlSpec, ControlTypeName, NormalizeHint } from './specs'
// ── Declarative control API ──────────────────────────────────────────
export { color, combo, hue, isControlSpec, num, text, toggle } from './specs'
