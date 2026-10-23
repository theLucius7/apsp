import { useEffect, useRef, useState } from 'react'
import { AnimatePresence, m } from 'motion/react'
import { SearchIcon, StarIcon, XIcon } from 'lucide-react'
import { api, errorMessage } from '../api'
import { usePreference } from './preferences'
import { useI18n } from '../i18n'
import { cn } from '@/lib/utils'
import { popoverMotion } from '@/lib/motion'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Kbd } from '@/components/ui/kbd'

type Item = { kind: string; id: number | string; label: string; path: string }
const kinds: Record<string, string> = { server: '服务器', ip: '服务器地址', 'ip-observation': 'IP来源观测', 'ip-quality': 'IP质量资料', node: '节点', 'proxy-user': '代理用户', domain: '域名', certificate: '证书', 'dns-rule': 'DNS规则', 'network-rule': '网络规则', 'diagnostic-task': '诊断任务', 'test-plan': '测试方案', 'test-run': '测试运行', 'command-task': '远程命令', 'fleet-task': '设备操作', 'operation-task': '运维任务', 'operation-schedule': '任务计划', 'remediation-rule': '自动处置规则', maintenance: '维护窗口', incident: '故障事件', 'probe-rule': '探测规则', 'alert-rule': '告警规则', 'latency-task': '延迟监测', task: '任务', rule: '规则' }

export default function GlobalSearch() {
  const { t } = useI18n()
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<Item[]>([])
  const [open, setOpen] = useState(false)
  const [error, setError] = useState('')
  const input = useRef<HTMLInputElement>(null)
  const recent = usePreference<Item[]>('recent')
  const favorite = usePreference<Item[]>('favorite')
  useEffect(() => {
    const keyboard = (event: KeyboardEvent) => {
      if ((event.key === 'k' && (event.metaKey || event.ctrlKey)) || (event.key === '/' && !(event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement))) { event.preventDefault(); setOpen(true); input.current?.focus() }
      if (event.key === 'Escape') setOpen(false)
    }
    window.addEventListener('keydown', keyboard)
    return () => window.removeEventListener('keydown', keyboard)
  }, [])
  useEffect(() => {
    if (!query.trim()) { setResults([]); return }
    const controller = new AbortController()
    const timer = window.setTimeout(() => {
      void api<{ results: Item[] }>(`/api/control-center/search?q=${encodeURIComponent(query.trim())}`, 'GET', undefined, controller.signal)
        .then(result => { setResults(result.results); setError('') }).catch(error => { if (!controller.signal.aborted) setError(errorMessage(error)) })
    }, 250)
    return () => { window.clearTimeout(timer); controller.abort() }
  }, [query])
  const visit = (item: Item) => {
    window.location.hash = item.path; setOpen(false)
    if (recent.ready) void recent.save([item, ...(recent.value ?? []).filter(value => value.path !== item.path)].slice(0, 20)).catch(error => setError(errorMessage(error)))
  }
  const pin = (item: Item) => {
    const values = favorite.value ?? []
    void favorite.save(values.some(value => value.path === item.path) ? values.filter(value => value.path !== item.path) : [item, ...values].slice(0, 30)).catch(error => setError(errorMessage(error)))
  }
  const shown = query.trim() ? results : [...(favorite.value ?? []), ...(recent.value ?? []).filter(item => !(favorite.value ?? []).some(value => value.path === item.path))]
  return <div data-slot="global-search" className="relative w-full min-w-0 sm:w-72 lg:w-80">
    <SearchIcon className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2" aria-hidden="true" />
    <Input ref={input} type="search" aria-label={t('全局搜索')} placeholder={t('搜索服务器、域名、节点、任务…')} value={query} className="pr-14 pl-8"
      onFocus={() => setOpen(true)} onChange={event => { setQuery(event.target.value); setOpen(true) }} />
    <Kbd className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 max-sm:hidden">⌘K</Kbd>
    <AnimatePresence>
      {open && <m.div {...popoverMotion} className="bg-popover text-popover-foreground ring-foreground/10 absolute top-[calc(100%+0.5rem)] right-0 z-30 grid max-h-[65dvh] w-[min(32rem,calc(100vw-2rem))] origin-top-right gap-2 overflow-y-auto rounded-xl p-3 shadow-lg ring-1">
        <div className="flex items-center justify-between gap-2">
          <strong className="text-sm font-medium">{query.trim() ? t('搜索结果') : t('收藏与最近访问')}</strong>
          <Button variant="ghost" size="icon-sm" onClick={() => setOpen(false)} aria-label={t('关闭搜索')}><XIcon /></Button>
        </div>
        {error && <p role="alert" className="text-destructive text-sm">{error}</p>}
        {shown.length === 0 && <p className="text-muted-foreground py-4 text-center text-sm">{query.trim() ? t('没有匹配的授权对象') : t('输入搜索词或访问对象后添加收藏。')}</p>}
        {shown.length > 0 && <ul className="grid gap-0.5">
          {shown.map((item, index) => {
            const pinned = (favorite.value ?? []).some(value => value.path === item.path)
            return <li key={`${item.kind}-${item.id}-${index}`} className="flex items-center gap-1">
              <button type="button" className="hover:bg-muted focus-visible:ring-ring/50 flex min-w-0 flex-1 items-center gap-3 rounded-lg px-2 py-2 text-left text-sm outline-none focus-visible:ring-3" onClick={() => visit(item)}>
                <small className="text-muted-foreground w-20 shrink-0 truncate text-xs">{t(kinds[item.kind] ?? item.kind)}</small><span className="min-w-0 break-words">{item.label}</span>
              </button>
              <Button variant="ghost" size="icon-sm" disabled={!favorite.ready} aria-label={`${t('收藏')}${item.label}`} aria-pressed={pinned} onClick={() => pin(item)}>
                <StarIcon className={cn(pinned && 'fill-warning text-warning')} />
              </Button>
            </li>
          })}
        </ul>}
        <small className="text-muted-foreground border-t pt-2 text-xs">{t('Ctrl / ⌘ + K 打开，Esc 关闭')}</small>
      </m.div>}
    </AnimatePresence>
  </div>
}
