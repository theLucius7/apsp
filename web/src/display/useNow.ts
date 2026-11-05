import { useEffect, useState } from 'react'

/**
 * The clock live readings are measured against. It only ticks while the page is
 * visible, and catches up at once when the page comes back.
 */
export function useNow(interval = 5000) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    const tick = () => { if (document.visibilityState === 'visible') setNow(Date.now()) }
    const timer = window.setInterval(tick, interval)
    document.addEventListener('visibilitychange', tick)
    return () => { window.clearInterval(timer); document.removeEventListener('visibilitychange', tick) }
  }, [interval])
  return now
}
