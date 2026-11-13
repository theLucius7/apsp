/** What a write guard needs from a read: the data, whether it is confirmed current, and its error. */
export type ResourceSnapshot<T> = { data?: T; fresh: boolean; error: string; isCurrent?: () => boolean; getCurrent?: () => T | undefined }

/**
 * A read narrowed to responses of the expected shape. A malformed response keeps the last
 * accepted data on screen, reports an error and is never current, so nothing writes from it.
 */
export function validatedSnapshot<T>(resource: ResourceSnapshot<unknown>, valid: (value: unknown) => value is T, previous?: T): ResourceSnapshot<T> {
  const accepted = valid(resource.data)
  const historical = accepted ? resource.data as T : previous
  const current = () => resource.getCurrent ? resource.getCurrent() : resource.data
  const fresh = () => (resource.isCurrent ? resource.isCurrent() : resource.fresh) && valid(current()) && !resource.error
  return {
    get data() { const value = current(); return valid(value) ? value : historical },
    get fresh() { return fresh() },
    get error() { const value = current(); return resource.error || (value !== undefined && !valid(value) || !accepted && resource.data !== undefined ? '面板返回的资源信息格式不完整，请刷新确认。' : '') },
    isCurrent: fresh,
    getCurrent: () => { const value = current(); return fresh() && valid(value) ? value : undefined },
  }
}

/** Whether a draft may still be written: the revision it was read at is a real one and is still the current one. */
export const revisionMatches = (draft: number | undefined, current: number | undefined) => Number.isSafeInteger(draft) && draft! > 0 && draft === current
