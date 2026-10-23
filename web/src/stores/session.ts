import { create } from 'zustand'

type SessionState = {
  /** null while the access check is in flight. */
  authenticated: boolean | null
  publicDashboard: boolean
  notice: string
  /** Bumped whenever access must be checked again. */
  revision: number
  setAccess: (access: { authenticated: boolean; publicDashboard: boolean }) => void
  signedIn: () => void
  /** Leaves the notice unchanged when none is given. */
  signedOut: (notice?: string) => void
  failed: (notice: string) => void
  clearNotice: () => void
}

/** Administrator session state shared by the router, login page and shell. */
export const useSessionStore = create<SessionState>()(set => ({
  authenticated: null,
  publicDashboard: false,
  notice: '',
  revision: 0,
  setAccess: ({ authenticated, publicDashboard }) => set({ authenticated, publicDashboard }),
  signedIn: () => set(state => ({ authenticated: true, notice: '', revision: state.revision + 1 })),
  signedOut: notice => set(state => ({ authenticated: false, publicDashboard: false, notice: notice ?? state.notice, revision: state.revision + 1 })),
  failed: notice => set({ authenticated: false, publicDashboard: false, notice }),
  clearNotice: () => set({ notice: '' }),
}))
