/** Remembers resizable layouts; the panels still work when storage is blocked. */
export const layoutStorage = {
  getItem: (key: string) => { try { return localStorage.getItem(key) } catch { return null } },
  setItem: (key: string, value: string) => { try { localStorage.setItem(key, value) } catch { /* Layout memory is optional. */ } },
}
