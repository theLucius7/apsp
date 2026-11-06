import { createContext } from 'react'
import type { LucideIcon } from 'lucide-react'
import type { NavigationItem } from './navigation'

/** The tabs of the sidebar entry the current page belongs to; the page header shows them. */
export const SectionContext = createContext<readonly NavigationItem[]>([])

/** The icon of the page being shown, as the sidebar draws it; the page header repeats it beside the title. */
export const PageIconContext = createContext<LucideIcon | undefined>(undefined)
