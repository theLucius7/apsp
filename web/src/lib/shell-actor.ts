// What the sidebar needs to know about the signed-in administrator, remembered so
// a reload draws the same navigation immediately instead of growing it after the
// first read. It decides only which links appear; the panel enforces every access.
export type ShellActor = { role: string; display_name: string; all_servers: boolean; capabilities: string[]; show_frozen_features?: boolean }

const STORAGE_KEY = 'apsp-shell-actor'

export function readShellActor(): ShellActor | undefined {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null') as ShellActor | null
    return value && typeof value.role === 'string' && Array.isArray(value.capabilities) ? value : undefined
  } catch { return undefined }
}

export function writeShellActor({ role, display_name, all_servers, capabilities, show_frozen_features }: ShellActor) {
  try { localStorage.setItem(STORAGE_KEY, JSON.stringify({ role, display_name, all_servers, capabilities, show_frozen_features })) } catch { /* Only the next reload is affected. */ }
}

export function clearShellActor() {
  try { localStorage.removeItem(STORAGE_KEY) } catch { /* Nothing was remembered. */ }
}
