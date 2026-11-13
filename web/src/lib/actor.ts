import { useEffect } from 'react'
import { create } from 'zustand'
import { settingsPoll, useResource } from '../hooks'
import type { ResourceState } from '../hooks'

/** The signed-in administrator, as the panel describes them. */
export type Actor = {
  admin_id: number
  login_name: string
  display_name: string
  role: string
  all_servers: boolean
  capabilities: string[]
  server_ids: number[]
  /** Panel-wide: whether frozen features are shown (面板设置). */
  show_frozen_features?: boolean
}

const unread: ResourceState<Actor> = { data: undefined, error: '', loading: true, ready: false, refreshing: true, fresh: false, reload: () => {}, isCurrent: () => false, getCurrent: () => undefined }
const useActorStore = create<ResourceState<Actor>>()(() => unread)

/**
 * Reads the signed-in administrator for the whole console. The application mounts
 * this once, while an administrator is signed in, and polls it like other settings;
 * pages read the same answer through useActor, so opening a page asks nothing again.
 */
export function useActorSource(signedIn: boolean) {
  const { data, error, loading, ready, refreshing, fresh, reload, isCurrent, getCurrent } = useResource<Actor>(signedIn ? '/api/control-center/me' : null, settingsPoll)
  useEffect(() => {
    useActorStore.setState({ data, error, loading, ready, refreshing, fresh, reload, isCurrent, getCurrent })
  }, [data, error, loading, ready, refreshing, fresh, reload, isCurrent, getCurrent])
}

/**
 * The signed-in administrator. It is a read like any other: write guards take it as
 * one of their resources, and `reload` reads it again for every page at once.
 */
export const useActor = (): ResourceState<Actor> => useActorStore()
