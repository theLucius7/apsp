// The small checks every response validator and form check is built from. Pages
// narrow what the panel sent with these before showing or writing from it.

/** A JSON object: not null and not an array. */
export const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value)

/** The object itself, or an empty one for anything else, so an optional report section reads field by field. */
export const recordOf = (value: unknown): Record<string, unknown> => isRecord(value) ? value : {}

/** The object has exactly these keys, written space separated, and no others. */
export const hasExactKeys = (value: Record<string, unknown>, keys: string) => Object.keys(value).length === keys.split(' ').length && keys.split(' ').every(key => Object.hasOwn(value, key))

/** A whole number JavaScript holds exactly, no smaller than `minimum`. */
export const isInteger = (value: unknown, minimum = 0): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= minimum

/** A TCP or UDP port, 1–65535. */
export const isPort = (value: unknown): value is number => isInteger(value, 1) && value <= 65535

/** A UUID of versions 1–5, the form the panel issues identifiers in. */
export const isUuid = (value: unknown): value is string => typeof value === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value)
