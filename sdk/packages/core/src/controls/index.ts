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
    RectControlDefinition,
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
export type {
    ControlSpec,
    ControlTypeName,
    FontOptions,
    NormalizeHint,
    PaletteControlOptions,
    RectOptions,
    RectValue,
    SensorOptions,
} from './specs'
// ── Declarative control API ──────────────────────────────────────────
export {
    color,
    combo,
    font,
    hue,
    isControlSpec,
    isPaletteControl,
    num,
    paletteControl,
    rect,
    sensor,
    text,
    toggle,
} from './specs'
