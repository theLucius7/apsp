import { create } from 'zustand'

type FeatureState = {
  /** Frozen features stay hidden until 面板设置 shows them. */
  showFrozen: boolean
  setShowFrozen: (showFrozen: boolean) => void
}

/** Panel-wide feature visibility, read by the admin shell from the panel settings. */
export const useFeatureStore = create<FeatureState>()(set => ({
  showFrozen: false,
  setShowFrozen: showFrozen => set({ showFrozen }),
}))
