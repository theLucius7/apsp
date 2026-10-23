import type { Transition } from 'motion/react'

// One place for every animation timing in the interface. Components import
// these presets instead of declaring their own durations or easings, and the
// root <MotionConfig reducedMotion="user"> turns movement off for visitors who
// ask their system for reduced motion.
export const duration = { fast: 0.12, base: 0.18, slow: 0.26 } as const

export const easing = {
  out: [0.16, 1, 0.3, 1],
  inOut: [0.65, 0, 0.35, 1],
} as const satisfies Record<string, readonly [number, number, number, number]>

/** The same timing for libraries that animate with CSS transitions (dnd-kit). */
export const cssTransition = { duration: Math.round(duration.base * 1000), easing: `cubic-bezier(${easing.out.join(', ')})` }

export const transition = {
  fast: { duration: duration.fast, ease: easing.out },
  base: { duration: duration.base, ease: easing.out },
  slow: { duration: duration.slow, ease: easing.inOut },
  spring: { type: 'spring', stiffness: 520, damping: 40, mass: 0.8 },
} as const satisfies Record<string, Transition>

/** Dimmed backdrop behind dialogs. */
export const fadeMotion = {
  initial: { opacity: 0 },
  animate: { opacity: 1 },
  exit: { opacity: 0 },
  transition: transition.fast,
} as const

/** Centered dialogs grow in slightly from their own center. */
export const dialogMotion = {
  initial: { opacity: 0, scale: 0.97 },
  animate: { opacity: 1, scale: 1 },
  exit: { opacity: 0, scale: 0.97 },
  transition: transition.base,
} as const

/** Menus, popovers and tooltips scale from their trigger side. */
export const popoverMotion = {
  initial: { opacity: 0, scale: 0.96 },
  animate: { opacity: 1, scale: 1 },
  exit: { opacity: 0, scale: 0.96 },
  transition: transition.fast,
} as const

/** Contextual action bars rise from the bottom edge while a selection exists. */
export const barMotion = {
  initial: { opacity: 0, y: 12 },
  animate: { opacity: 1, y: 0 },
  exit: { opacity: 0, y: 12 },
  transition: transition.fast,
} as const

/** Page content settles in after navigation. */
export const pageMotion = {
  initial: { opacity: 0, y: 6 },
  animate: { opacity: 1, y: 0 },
  transition: transition.base,
} as const

/** Inline notices and expanding sections reveal their height. */
export const revealMotion = {
  initial: { opacity: 0, height: 0 },
  animate: { opacity: 1, height: 'auto' },
  exit: { opacity: 0, height: 0 },
  transition: transition.base,
} as const

/** List items that join a list; filtered-out items leave at once. */
export const itemMotion = {
  initial: { opacity: 0, y: 4 },
  animate: { opacity: 1, y: 0 },
  transition: transition.fast,
} as const
