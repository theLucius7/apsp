import { useEffect, useId, useMemo, useState } from 'react'
import type { KeyboardEvent as ReactKeyboardEvent } from 'react'
import type { LucideIcon } from 'lucide-react'
import { BellRingIcon, ClipboardListIcon, FileTextIcon, GlobeIcon, LayoutGridIcon, SearchIcon, ServerIcon, ShieldCheckIcon, StarIcon, UsersIcon, WaypointsIcon } from 'lucide-react'
import { api, errorMessage } from '../api'
import { confirmLeave, usePreference } from './preferences'
import { useI18n } from '../i18n'
import type { NavigationItem } from '../app/navigation'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Kbd } from '@/components/ui/kbd'
import { Spinner } from '@/components/ui/spinner'

type Item = { kind: string; id: number | string; label: string; path: string }
type Entry = { key: string; label: string; detail: string; icon: LucideIcon; item: Item }
type Section = { id: string; title: string; entries: Entry[] }

const kinds: Record<string, string> = { page: '页面', server: '服务器', ip: '服务器地址', 'ip-observation': 'IP来源观测', 'ip-quality': 'IP质量资料', node: '节点', 'proxy-user': '代理用户', domain: '域名', certificate: '证书', 'dns-rule': 'DNS规则', 'network-rule': '网络规则', 'diagnostic-task': '诊断任务', 'test-plan': '测试方案', 'test-run': '测试运行', 'command-task': '远程命令', 'fleet-task': '设备操作', 'operation-task': '运维任务', 'operation-schedule': '任务计划', 'remediation-rule': '自动处置规则', maintenance: '维护窗口', incident: '故障事件', 'probe-rule': '探测规则', 'alert-rule': '告警规则', 'latency-task': '延迟监测', task: '任务', rule: '规则' }
const kindIcons: Record<string, LucideIcon> = { server: ServerIcon, ip: ServerIcon, node: WaypointsIcon, 'proxy-user': UsersIcon, domain: GlobeIcon, 'dns-rule': GlobeIcon, certificate: ShieldCheckIcon, incident: BellRingIcon, 'alert-rule': BellRingIcon }
const taskKinds = new Set(['diagnostic-task', 'test-plan', 'test-run', 'command-task', 'fleet-task', 'operation-task', 'operation-schedule', 'task'])

// Field where typing "/" must stay text instead of opening the palette, including
// custom text surfaces such as the remote terminal.
const editable = (target: EventTarget | null) => target instanceof HTMLElement && (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName) || target.closest('[role="textbox"]') !== null)

/**
 * Keyboard-first jump list for the administrator console: permitted pages,
 * favourites, recent visits and server-side search results in one listbox.
 */
export default function CommandPalette({ pages }: { pages: readonly NavigationItem[] }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<Item[]>([])
  const [searching, setSearching] = useState(false)
  const [error, setError] = useState('')
  // The active option is tracked by key so late search results never move the keyboard selection.
  const [activeKey, setActiveKey] = useState<string>()
  const id = useId()
  const recent = usePreference<Item[]>('recent')
  const favorite = usePreference<Item[]>('favorite')
  const term = query.trim().toLowerCase()

  useEffect(() => {
    // The palette does not open over another dialog, where a draft may be in progress.
    const covered = () => document.querySelector('[role="dialog"], [role="alertdialog"]') !== null
    const keyboard = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() === 'k' && (event.metaKey || event.ctrlKey)) { event.preventDefault(); setOpen(value => value ? false : !covered()) }
      else if (event.key === '/' && !event.metaKey && !event.ctrlKey && !event.altKey && !editable(event.target) && !covered()) { event.preventDefault(); setOpen(true) }
    }
    window.addEventListener('keydown', keyboard)
    return () => window.removeEventListener('keydown', keyboard)
  }, [])

  useEffect(() => {
    if (!open || !term) { setResults([]); setSearching(false); return }
    const controller = new AbortController()
    setSearching(true)
    const timer = window.setTimeout(() => {
      void api<{ results: Item[] }>(`/api/control-center/search?q=${encodeURIComponent(query.trim())}`, 'GET', undefined, controller.signal)
        .then(result => { setResults(result.results); setError('') })
        .catch(reason => { if (!controller.signal.aborted) setError(errorMessage(reason)) })
        .finally(() => { if (!controller.signal.aborted) setSearching(false) })
    }, 250)
    return () => { window.clearTimeout(timer); controller.abort() }
  }, [open, term, query])

  const pageIcon = (path: string) => pages.find(page => page.path === path)?.icon ?? LayoutGridIcon
  const entryOf = (item: Item, prefix: string): Entry => ({
    key: `${prefix}:${item.kind}:${item.id}:${item.path}`,
    label: item.kind === 'page' ? t(item.label) : item.label,
    detail: t(kinds[item.kind] ?? item.kind),
    icon: item.kind === 'page' ? pageIcon(item.path) : kindIcons[item.kind] ?? (taskKinds.has(item.kind) ? ClipboardListIcon : FileTextIcon),
    item,
  })
  const pageItem = (page: NavigationItem): Item => ({ kind: 'page', id: page.page, label: page.label, path: page.path })

  const sections = useMemo<Section[]>(() => {
    const matches = (item: Item) => !term || `${item.label} ${t(item.label)} ${item.path}`.toLowerCase().includes(term)
    const favourites = (favorite.value ?? []).filter(matches)
    const pinnedPaths = new Set(favourites.map(item => item.path))
    const recents = (recent.value ?? []).filter(item => !pinnedPaths.has(item.path) && matches(item)).slice(0, term ? 5 : 8)
    const pageMatches = pages.filter(page => !term || `${page.label} ${t(page.label)} ${page.group} ${t(page.group)} ${page.path}`.toLowerCase().includes(term))
    return [
      { id: 'favorite', title: t('收藏'), entries: favourites.map(item => entryOf(item, 'favorite')) },
      { id: 'recent', title: t('最近访问'), entries: recents.map(item => entryOf(item, 'recent')) },
      { id: 'pages', title: t('页面'), entries: pageMatches.map(page => ({ ...entryOf(pageItem(page), 'page'), detail: t(page.group), icon: page.icon })) },
      { id: 'results', title: t('搜索结果'), entries: term ? results.map(item => entryOf(item, 'result')) : [] },
    ].filter(section => section.entries.length > 0)
  }, [term, favorite.value, recent.value, pages, results, t])
  const entries = sections.flatMap(section => section.entries)
  const keyedIndex = entries.findIndex(entry => entry.key === activeKey)
  const activeIndex = entries.length ? Math.max(keyedIndex, 0) : -1
  const current = activeIndex >= 0 ? entries[activeIndex] : undefined
  const optionId = (index: number) => `${id}-option-${index}`
  // Index of each section's first option in the flattened list.
  const offsets = sections.map((_, index) => sections.slice(0, index).reduce((total, section) => total + section.entries.length, 0))

  // A new query starts again from the first match.
  useEffect(() => { setActiveKey(undefined) }, [term])
  useEffect(() => {
    if (open && activeIndex >= 0) document.getElementById(`${id}-option-${activeIndex}`)?.scrollIntoView({ block: 'nearest' })
  }, [activeIndex, open, id])

  const close = (next: boolean) => { setOpen(next); if (!next) { setQuery(''); setError(''); setActiveKey(undefined) } }
  const visit = (entry: Entry) => {
    if (!confirmLeave()) return
    window.location.hash = entry.item.path
    close(false)
    if (recent.ready) void recent.save([entry.item, ...(recent.value ?? []).filter(value => value.path !== entry.item.path)].slice(0, 20)).catch(reason => setError(errorMessage(reason)))
  }
  const pinned = (item?: Item) => Boolean(item && (favorite.value ?? []).some(value => value.path === item.path))
  const pin = (item: Item) => {
    const values = favorite.value ?? []
    void favorite.save(pinned(item) ? values.filter(value => value.path !== item.path) : [item, ...values].slice(0, 30)).catch(reason => setError(errorMessage(reason)))
  }
  const keyboard = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    // IME candidate selection owns these keys, including the legacy composition signal.
    if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return
    if (!entries.length) return
    if (event.key === 'ArrowDown') { event.preventDefault(); setActiveKey(entries[(activeIndex + 1) % entries.length].key) }
    else if (event.key === 'ArrowUp') { event.preventDefault(); setActiveKey(entries[(activeIndex - 1 + entries.length) % entries.length].key) }
    else if (event.key === 'Enter' && current) {
      event.preventDefault()
      if (event.shiftKey) { if (favorite.ready) pin(current.item) } else visit(current)
    }
  }

  return <>
    <Button data-slot="global-search" variant="outline" aria-haspopup="dialog" aria-keyshortcuts="Control+K Meta+K /" aria-label={t('全局搜索')} onClick={() => setOpen(true)}
      className="text-muted-foreground hover:text-foreground w-full min-w-0 justify-start font-normal sm:w-auto sm:min-w-40 sm:max-w-72 sm:flex-1 lg:max-w-80 max-sm:w-auto">
      <SearchIcon aria-hidden="true" /><span className="truncate max-sm:sr-only">{t('搜索或跳转…')}</span>
      <Kbd className="ml-auto max-sm:hidden">⌘K</Kbd>
    </Button>
    <Dialog open={open} onOpenChange={close}>
      <DialogContent data-slot="command-palette" showCloseButton={false} className="top-[12vh] translate-y-0 gap-0 overflow-hidden p-0 sm:max-w-xl">
        <DialogTitle className="sr-only">{t('搜索或跳转')}</DialogTitle>
        <DialogDescription className="sr-only">{t('输入页面名称或服务器、域名、节点等对象，用方向键选择，回车打开。')}</DialogDescription>
        <div className="flex items-center gap-2 border-b px-3">
          <SearchIcon className="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
          <input role="combobox" aria-expanded="true" aria-controls={`${id}-list`} aria-autocomplete="list" aria-activedescendant={current ? optionId(activeIndex) : undefined}
            aria-label={t('搜索页面或对象')} placeholder={t('搜索页面、服务器、域名、节点、任务…')} value={query} onChange={event => setQuery(event.target.value)} onKeyDown={keyboard}
            className="placeholder:text-muted-foreground h-12 min-w-0 flex-1 bg-transparent text-sm outline-none" />
          {searching && <Spinner className="text-muted-foreground" aria-hidden="true" role={undefined} aria-label={undefined} />}
        </div>
        {error && <p role="alert" className="text-destructive border-b px-4 py-2 text-sm">{error}</p>}
        <div id={`${id}-list`} role="listbox" aria-label={t('搜索结果')} className="max-h-[min(60dvh,26rem)] overflow-y-auto overscroll-contain p-1.5 scrollbar-thin">
          {sections.map((section, sectionIndex) => <div key={section.id} role="presentation" className="pb-1">
            <div id={`${id}-${section.id}`} aria-hidden="true" className="text-muted-foreground px-2 pt-2 pb-1 text-xs font-medium">{section.title}</div>
            <div role="group" aria-labelledby={`${id}-${section.id}`}>
              {section.entries.map((entry, entryIndex) => {
                const index = offsets[sectionIndex] + entryIndex, selected = index === activeIndex
                const Icon = entry.icon
                return <div key={entry.key} id={optionId(index)} role="option" aria-selected={selected} onMouseMove={() => { if (!selected) setActiveKey(entry.key) }} onClick={() => visit(entry)}
                  className="aria-selected:bg-accent aria-selected:text-accent-foreground flex cursor-pointer items-center gap-3 rounded-lg px-2 py-2 text-sm pointer-coarse:py-3">
                  <Icon className="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate">{entry.label}</span>
                  {pinned(entry.item) && <StarIcon className="fill-warning text-warning size-3.5 shrink-0" aria-label={t('已收藏')} />}
                  <small className="text-muted-foreground shrink-0 text-xs">{entry.detail}</small>
                </div>
              })}
            </div>
          </div>)}
        </div>
        {!entries.length && <p role="status" className="text-muted-foreground px-4 pb-8 pt-6 text-center text-sm">{term ? searching ? t('正在搜索…') : t('没有匹配的页面或对象') : t('输入搜索词，或从页面开始。')}</p>}
        <div className="text-muted-foreground flex items-center justify-between gap-3 border-t px-3 py-2 text-xs">
          <span className="flex items-center gap-3 max-sm:hidden">
            <span className="flex items-center gap-1"><Kbd>↑</Kbd><Kbd>↓</Kbd>{t('选择')}</span>
            <span className="flex items-center gap-1"><Kbd>Enter</Kbd>{t('打开')}</span>
            <span className="flex items-center gap-1"><Kbd>Esc</Kbd>{t('关闭')}</span>
          </span>
          <Button variant="ghost" size="xs" className="ml-auto" disabled={!current || !favorite.ready} aria-keyshortcuts="Shift+Enter" onClick={() => { if (current) pin(current.item) }}>
            <StarIcon className={cn(pinned(current?.item) && 'fill-warning text-warning')} aria-hidden="true" />
            {pinned(current?.item) ? t('取消收藏') : t('收藏当前项')}<Kbd className="max-sm:hidden">⇧ Enter</Kbd>
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  </>
}
