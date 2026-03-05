/**
 * Control system type definitions.
 * Defines interfaces for effect controls that map to HTML <meta> tags.
 */

/** Base interface for all control definitions. */
export interface ControlDefinition {
    id: string
    type: string
    label: string
    default: unknown
    tooltip?: string
    [key: string]: unknown
}

/** Number slider control. */
export interface NumberControlDefinition extends ControlDefinition {
    type: 'number'
    min: number
    max: number
    default: number
    step?: number
}

/** Boolean toggle control. */
export interface BooleanControlDefinition extends ControlDefinition {
    type: 'boolean'
    default: boolean | number
}

/** Dropdown combobox control. */
export interface ComboboxControlDefinition extends ControlDefinition {
    type: 'combobox'
    values: string[]
    default: string
}

/** Hue picker control (0-360). */
export interface HueControlDefinition extends ControlDefinition {
    type: 'hue'
    min: number
    max: number
    default: number
}

/** Color picker control (hex string). */
export interface ColorControlDefinition extends ControlDefinition {
    type: 'color'
    default: string
}

/** Text input control. */
export interface TextFieldControlDefinition extends ControlDefinition {
    type: 'textfield'
    default: string
}

/** Union of all control definition types. */
export type ControlDefinitionType =
    | NumberControlDefinition
    | BooleanControlDefinition
    | ComboboxControlDefinition
    | HueControlDefinition
    | ColorControlDefinition
    | TextFieldControlDefinition

/** Runtime control values dictionary. */
export interface ControlValues {
    [key: string]: unknown
}

/** Base interface that effect control types should extend. */
export interface BaseControls {
    [key: string]: unknown
}
