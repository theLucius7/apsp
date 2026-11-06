import { useState } from 'react'
import { AnimatePresence, m } from 'motion/react'
import { BellIcon, CalendarClockIcon, ListFilterIcon, PlusIcon, SearchIcon, UploadIcon } from 'lucide-react'
import { Empty } from '../../components'
import { money } from '../../display/currency'
import { toneFill, toneText } from '../../display/ui'
import { ActionMenu } from '@/components/action-menu'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { revealMotion } from '@/lib/motion'
import { cn } from '@/lib/utils'
import { SOON_DAYS, savedSort, selectRows } from './model'
import type { KindFilter, LedgerRow, LedgerSort } from './model'
import type { ItemOperation } from './operations'
import type { Expense } from './types'
import type { Ledger } from './useLedger'

type Handlers = { onOpen: (row: LedgerRow) => void; onEdit: (item: Expense) => void; onOperate: (item: Expense, kind: ItemOperation) => void }

function read(key: string) { try { return localStorage.getItem(`apsp-expenses-${key}`) } catch { return null } }
function save(key: string, value: string) { try { localStorage.setItem(`apsp-expenses-${key}`, value) } catch { /* The choice stays for this visit. */ } }

const columns = 'md:grid-cols-[minmax(0,2.4fr)_minmax(0,1.5fr)_minmax(0,1.1fr)_minmax(0,0.9fr)_minmax(9rem,auto)]'

/** One entry: the name opens its detail; renewing something due is one click away. */
function Row({ row, base, writable, onOpen, onEdit, onOperate }: Handlers & { row: LedgerRow; base?: string; writable: boolean }) {
  const item = row.kind === 'item' ? row.item : null
  // Manual renewals that are due are the one thing to do here; automatic ones move on by themselves.
  const due = !row.inactive && !row.auto && row.due.days !== null && row.due.days <= SOON_DAYS
  const subtitle = row.kind === 'server' ? '服务器' : [row.category, row.payment].filter(Boolean).join(' · ') || '未分类'
  const converted = row.monthly !== null && base && row.currency !== base ? `约 ${money(row.monthly, base)} / 月` : ''
  return <li data-slot="ledger-row" data-kind={row.kind} data-inactive={row.inactive || undefined}
    className={cn('hover:bg-muted/40 grid items-center gap-x-4 gap-y-1 px-4 py-3 transition-colors max-md:grid-cols-[minmax(0,1fr)_auto] max-md:px-3', columns, row.inactive && 'text-muted-foreground')}>
    <button type="button" onClick={() => onOpen(row)} aria-label={`${row.name}，${row.due.status.label}，查看详情`}
      className="focus-visible:shadow-focus group/name flex min-w-0 items-center gap-2.5 rounded-md text-left outline-none max-md:order-1">
      <span className={cn('size-2 flex-none rounded-full', toneFill[row.due.status.tone])} title={row.due.status.label} aria-hidden="true" />
      <span className="grid min-w-0">
        <strong className="truncate text-[13px] font-medium group-hover/name:underline">{row.name}</strong>
        <small className="text-muted-foreground truncate text-xs">{subtitle}</small>
      </span>
    </button>
    <div className="grid min-w-0 max-md:order-3 max-md:pl-[18px]">
      <span className="truncate text-[13px] tabular-nums">{row.price ?? '未填写价格'}<span className="text-muted-foreground text-xs"> / {row.cycle.replace(/^每\s*/, '')}</span></span>
      {converted && <small className="text-muted-foreground truncate text-xs tabular-nums">{converted}</small>}
    </div>
    <div className="grid min-w-0 max-md:order-4 max-md:text-right">
      <span className="text-[13px] tabular-nums">{row.inactive ? '已停用' : row.next}</span>
      {!row.inactive && row.due.days !== null && <small className={cn('text-xs tabular-nums', row.due.next.tone === 'muted' ? 'text-muted-foreground' : toneText[row.due.next.tone])}>{row.due.days <= SOON_DAYS ? row.due.next.label : `${row.due.days} 天后`}</small>}
    </div>
    <div className="text-muted-foreground flex min-w-0 items-center gap-1.5 text-xs max-md:hidden">
      <span className={cn('truncate', row.due.status.tone !== 'good' && toneText[row.due.status.tone])}>{row.due.status.label}</span>
      {item?.notify && !row.inactive && <BellIcon className="size-3 flex-none" aria-label={`提前 ${item.notify_days_before} 天提醒`} />}
    </div>
    <div className="flex items-center justify-end gap-1 max-md:order-2">
      {item && due && <Button variant="outline" size="sm" disabled={!writable} onClick={() => onOperate(item, 'renew')}>确认续期</Button>}
      {row.kind === 'server' && <Button variant="ghost" size="sm" onClick={() => onOpen(row)}>费用记录</Button>}
      {item ? <ActionMenu label={`${row.name} 的更多操作`} groups={[
        [{ label: '编辑', disabled: !writable, onSelect: () => onEdit(item) }, !due && { label: '确认续期', disabled: !writable, onSelect: () => onOperate(item, 'renew') }, { label: '复制', disabled: !writable, onSelect: () => onOperate(item, 'clone') }],
        [{ label: row.inactive ? '恢复使用' : '停用', disabled: !writable, onSelect: () => onOperate(item, 'toggle') }, { label: '删除', destructive: true, disabled: !writable, onSelect: () => onOperate(item, 'delete') }],
      ]} /> : <span className="w-8 max-md:hidden" aria-hidden="true" />}
    </div>
  </li>
}

/** What is paid for: hand-added services and servers in one list, the soonest renewal first. */
export default function LedgerList({ ledger, onAdd, onImport, ...handlers }: Handlers & { ledger: Ledger; onAdd: () => void; onImport: () => void }) {
  const [query, setQuery] = useState('')
  const [kind, setKind] = useState<KindFilter>('all')
  const [category, setCategory] = useState('')
  const [inactive, setInactive] = useState(false)
  const [sort, setSort] = useState<LedgerSort>(() => savedSort(read('sort')))
  const [optionsOpen, setOptionsOpen] = useState(false)
  const rows = selectRows(ledger.rows, { query, kind, inactive, category: kind === 'server' ? '' : category, sort })
  const shown = ledger.rows.filter(row => inactive || !row.inactive)
  const counts = { all: shown.length, item: shown.filter(row => row.kind === 'item').length, server: shown.filter(row => row.kind === 'server').length }
  const categories = ledger.labels.filter(label => label.kind === 'category')
  const stopped = ledger.rows.filter(row => row.inactive).length
  const changed = Boolean(query) || kind !== 'all' || Boolean(category)
  const customised = Boolean(category) || inactive || sort !== 'next'
  const reset = () => { setQuery(''); setKind('all'); setCategory('') }
  if (ledger.loaded && !ledger.rows.length) return <Empty icon={CalendarClockIcon} title="还没有续费项目" description="添加域名、AI 与其他按周期付费的服务；设置了到期日期的服务器会自动列在这里。">
    <Button onClick={onAdd}><PlusIcon aria-hidden="true" />添加项目</Button>
    <Button variant="outline" onClick={onImport}><UploadIcon aria-hidden="true" />从 Wallos 导入</Button>
  </Empty>
  return <section aria-label="续费项目" className="grid min-w-0 gap-3">
    <div data-slot="ledger-toolbar" className="min-w-0">
      <div className="flex min-h-9 items-center gap-3 max-lg:flex-wrap max-lg:gap-2">
        <label className="relative flex min-w-44 flex-[0_1_300px] items-center max-lg:flex-[1_1_180px] max-sm:min-w-24">
          <SearchIcon className="text-muted-foreground pointer-events-none absolute left-2.5 size-4" aria-hidden="true" />
          <Input type="search" aria-label="搜索项目" placeholder="搜索名称、分类、备注" value={query} onChange={event => setQuery(event.target.value)} className="pl-8" />
        </label>
        <SegmentedGroup aria-label="项目类型" className="no-scrollbar max-w-full flex-nowrap overflow-x-auto *:shrink-0 max-lg:order-3 max-lg:flex-[1_1_100%]">
          {([['all', '全部'], ['item', '服务'], ['server', '服务器']] as const).map(([value, label]) => <SegmentedButton key={value} aria-pressed={kind === value} onClick={() => setKind(value)}>{label}<span className="text-muted-foreground text-xs tabular-nums" aria-hidden="true">{counts[value]}</span></SegmentedButton>)}
        </SegmentedGroup>
        <div className="ml-auto flex flex-none items-center gap-1">
          <span className="text-muted-foreground mr-1.5 text-xs whitespace-nowrap">{ledger.loaded ? `${rows.length} 项` : '等待数据'}</span>
          <Button variant="ghost" size="icon" className={cn(customised && 'bg-primary/10 text-primary')} aria-label="筛选与排序" title="筛选与排序" aria-expanded={optionsOpen} aria-controls="ledger-options" onClick={() => setOptionsOpen(value => !value)}><ListFilterIcon /></Button>
        </div>
      </div>
      <AnimatePresence initial={false}>
        {optionsOpen && <m.div key="options" {...revealMotion} className="overflow-hidden">
          <div id="ledger-options" className="text-muted-foreground mt-3 flex flex-wrap items-center gap-x-5 gap-y-3 border-t pt-3 text-xs">
            {categories.length > 0 && kind !== 'server' && <label className="flex items-center gap-2">分类<NativeSelect size="sm" aria-label="分类" value={category} onChange={event => setCategory(event.target.value)} className="max-w-40"><option value="">全部分类</option>{categories.map(label => <option key={label.id} value={label.id}>{label.name}</option>)}</NativeSelect></label>}
            <label className="flex items-center gap-2">排序<NativeSelect size="sm" aria-label="排序" value={sort} onChange={event => { const next = savedSort(event.target.value); setSort(next); save('sort', next) }} className="max-w-40"><option value="next">下次续费近的在前</option><option value="cost">每月费用高的在前</option><option value="name">按名称</option></NativeSelect></label>
            <label className="flex cursor-pointer items-center gap-2"><Switch size="sm" checked={inactive} onCheckedChange={setInactive} aria-label="显示已停用" />显示已停用{stopped > 0 && <span className="tabular-nums">（{stopped}）</span>}</label>
            {changed && <Button variant="link" size="sm" className="ml-auto" onClick={reset}>清除筛选</Button>}
          </div>
        </m.div>}
      </AnimatePresence>
    </div>
    <div className="bg-card text-card-foreground shadow-soft min-w-0 overflow-hidden rounded-xl border">
      <div aria-hidden="true" className={cn('text-muted-foreground grid gap-x-4 border-b px-4 py-2 text-xs max-md:hidden', columns)}><span>项目</span><span>费用</span><span>下次续费</span><span>方式</span><span /></div>
      {!ledger.loaded ? <p role="status" className="text-muted-foreground px-4 py-8 text-center text-sm">正在读取…</p>
        : rows.length ? <ul className="divide-y">{rows.map(row => <Row key={row.key} row={row} base={ledger.base} writable={!ledger.stale} {...handlers} />)}</ul>
          : <div className="text-muted-foreground grid justify-items-center gap-2 px-4 py-8 text-center text-sm"><span>没有匹配的项目</span>{changed && <Button variant="outline" size="sm" onClick={reset}>清除筛选</Button>}</div>}
    </div>
  </section>
}
