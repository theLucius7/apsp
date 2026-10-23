import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { LazyMotion, MotionConfig } from 'motion/react'
import App from './App'
import { I18nProvider } from './i18n'
import './index.css'

// Animation features load in their own chunk; components use the lightweight
// `m` elements (strict mode rejects the full `motion` elements).
const motionFeatures = () => import('./lib/motion-features').then(module => module.default)

createRoot(document.getElementById('root')!).render(<StrictMode>
  <LazyMotion features={motionFeatures} strict>
    <MotionConfig reducedMotion="user">
      <I18nProvider><App /></I18nProvider>
    </MotionConfig>
  </LazyMotion>
</StrictMode>)
