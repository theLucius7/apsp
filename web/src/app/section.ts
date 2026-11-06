import { createContext } from 'react'
import type { NavigationItem } from './navigation'

/** The tabs of the sidebar entry the current page belongs to; the page header shows them in place of its name. */
export const SectionContext = createContext<readonly NavigationItem[]>([])
