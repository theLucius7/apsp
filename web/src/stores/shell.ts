import { create } from 'zustand'

type ShellState = {
  /** Desktop sidebar collapsed to its icon rail. */
  collapsed: boolean
  /** Compact-screen navigation sheet. */
  mobileOpen: boolean
  setCollapsed: (collapsed: boolean) => void
  setMobileOpen: (open: boolean) => void
}

const STORAGE_KEY = 'apsp-shell-collapsed'

function readCollapsed() {
  try { return localStorage.getItem(STORAGE_KEY) === 'true' } catch { return false }
}

/** Administrator shell layout shared by the sidebar, top bar and page content. */
export const useShellStore = create<ShellState>()(set => ({
  collapsed: readCollapsed(),
  mobileOpen: false,
  // Only an actual change of the collapse preference touches browser storage.
  setCollapsed: collapsed => set(state => {
    if (state.collapsed === collapsed) return state
    try { localStorage.setItem(STORAGE_KEY, String(collapsed)) } catch { /* The in-memory choice still applies. */ }
    return { collapsed }
  }),
  setMobileOpen: mobileOpen => set({ mobileOpen }),
}))
