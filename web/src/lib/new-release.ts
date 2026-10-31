const STORAGE_KEY = 'apsp-release-reload'
const minimumInterval = 60_000

/**
 * A code chunk that cannot load usually means a new release replaced it while this
 * tab stayed open. Reloads once when the panel is reachable; returns whether it did,
 * so a real network problem or a repeat failure can show an error instead of looping.
 */
export async function reloadForNewRelease() {
  try {
    if (Date.now() - Number(sessionStorage.getItem(STORAGE_KEY) ?? 0) < minimumInterval) return false
    const response = await fetch(window.location.pathname, { cache: 'no-store' })
    if (!response.ok) return false
    sessionStorage.setItem(STORAGE_KEY, String(Date.now()))
  } catch { return false }
  window.location.reload()
  return true
}

/** Dynamic import failures have no error type of their own across browsers. */
export const isChunkLoadError = (error: unknown) => error instanceof Error
  && /dynamically imported module|Importing a module script failed|error loading dynamically imported|Failed to fetch dynamically/i.test(error.message)
