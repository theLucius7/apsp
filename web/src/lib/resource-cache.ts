// The last reading of each list a page opted into. Returning to a page paints this
// at once instead of a skeleton, while the fresh read runs behind it. It only ever
// fills the screen: write guards treat it as unconfirmed until the panel answers.
// The cache lives in memory and is forgotten whenever the session changes.
const kept = new Map<string, unknown>()

export const readKept = <T,>(path: string) => kept.get(path) as T | undefined
export const writeKept = (path: string, value: unknown) => { if (value === undefined) kept.delete(path); else kept.set(path, value) }
export const clearKept = () => kept.clear()
