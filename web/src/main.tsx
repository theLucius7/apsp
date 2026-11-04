import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { LazyMotion, MotionConfig, domAnimation } from 'motion/react'
import App from './App'
import { I18nProvider } from './i18n'
import './index.css'
import { installConnectionMonitor } from './stores/connection'
import { useSessionStore } from './stores/session'

installConnectionMonitor(() => useSessionStore.getState().authenticated === true)

// Animation features ship with the first load (in the eager "motion" vendor
// chunk): an animation runtime that arrives late or fails to load must never
// leave content in its initial state. Components use the lightweight `m`
// elements (strict mode rejects the full `motion` elements). No component uses
// layout or drag animations (dnd-kit reorders lists), so domAnimation suffices.
createRoot(document.getElementById('root')!).render(<StrictMode>
  <LazyMotion features={domAnimation} strict>
    <MotionConfig reducedMotion="user">
      <I18nProvider><App /></I18nProvider>
    </MotionConfig>
  </LazyMotion>
</StrictMode>)
