import { create } from 'zustand'
import { clearKept } from '@/lib/resource-cache'
import { clearShellActor } from '@/lib/shell-actor'

type SessionState = {
  /** null while the access check is in flight. */
  authenticated: boolean | null
  publicDashboard: boolean
  notice: string
  /** Why the first access check has no answer yet; it keeps retrying meanwhile. */
  bootError: string
  /** Bumped whenever access must be checked again. */
  revision: number
  setAccess: (access: { authenticated: boolean; publicDashboard: boolean }) => void
  signedIn: () => void
  /** Leaves the notice unchanged when none is given. */
  signedOut: (notice?: string) => void
  /** The access check got no usable answer: this says nothing about the session. */
  failed: (message: string) => void
  clearNotice: () => void
}

/** Administrator session state shared by the router, login page and shell. */
export const useSessionStore = create<SessionState>()(set => ({
  authenticated: null,
  publicDashboard: false,
  notice: '',
  bootError: '',
  revision: 0,
  setAccess: ({ authenticated, publicDashboard }) => set({ authenticated, publicDashboard, bootError: '' }),
  signedIn: () => { clearKept(); set(state => ({ authenticated: true, notice: '', bootError: '', revision: state.revision + 1 })) },
  signedOut: notice => { clearKept(); clearShellActor(); set(state => ({ authenticated: false, publicDashboard: false, notice: notice ?? state.notice, bootError: '', revision: state.revision + 1 })) },
  failed: message => set(state => state.authenticated === null ? { bootError: message } : state),
  clearNotice: () => set({ notice: '' }),
}))
