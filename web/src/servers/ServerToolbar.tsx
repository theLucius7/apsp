import { ListFilterIcon, SearchIcon, Settings2Icon, XIcon } from 'lucide-react'
import { usePreference } from '../control-center/preferences'
import { useAction } from '../hooks'
import { ErrorNotice, Field } from '../components'
import { ToolbarPanel } from '@/components/toolbar-panel'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'

/**
 * How the administrator looks at the server list. `columns` are the desktop table
 * columns; `cardColumns` are the fields phone cards show, fewer by default so a
 * phone first answers which server, whether it is online, and where its details are.
 */
export type ServerView = {
  query: string; online: 'all' | 'online' | 'offline'; group: string; region: string
  groupBy: 'none' | 'group' | 'region'; sort: 'id' | 'name' | 'cpu' | 'expiry'
  columns: string[]; cardColumns: string[]; density: 'comfortable' | 'compact'
}
export const defaultServerView: ServerView = { query: '', online: 'all', group: '', region: '', groupBy: 'none', sort: 'id', columns: ['cpu', 'memory', 'cost', 'traffic', 'version'], cardColumns: ['cpu', 'memory'], density: 'comfortable' }
export const serverFilters = (view: ServerView) => ({ query: Boolean(view.query), online: view.online !== 'all', more: Number(Boolean(view.group)) + Number(Boolean(view.region)) })
export const clearServerFilters = (view: ServerView): ServerView => ({ ...view, query: '', online: 'all', group: '', region: '' })

const fields: [string, string][] = [['cpu', '处理器'], ['memory', '内存'], ['cost', '成本与到期'], ['traffic', '流量'], ['version', '设备版本']]

function FilterChip({ label, onRemove }: { label: string; onRemove: () => void }) {
  return <span data-slot="filter-chip" className="bg-muted text-foreground inline-flex h-7 items-center gap-0.5 rounded-full py-0.5 pr-0.5 pl-2.5 text-xs">
    {label}<Button variant="ghost" size="icon-xs" className="rounded-full" aria-label={`移除筛选条件：${label}`} onClick={onRemove}><XIcon aria-hidden="true" /></Button>
  </span>
}

/**
 * The server list's filters in three layers: an always-visible row with search,
 * status, more filters and view settings; more filters (group, region) shown as
 * removable chips once in use; and view settings (sort, grouping, fields, row
 * density, saved views). `phone` says whether the list currently shows cards.
 */
export default function ServerToolbar({ value, onChange, groups, regions, phone }: { value: ServerView; onChange: (value: ServerView) => void; groups: string[]; regions: string[]; phone: boolean }) {
  const saved = usePreference<ServerView>('view:servers')
  const action = useAction()
  const set = (patch: Partial<ServerView>) => onChange({ ...value, ...patch })
  const active = serverFilters(value)
  const shown = phone ? value.cardColumns : value.columns
  const toggle = (key: string, on: boolean) => {
    const next = on ? [...shown, key] : shown.filter(item => item !== key)
    set(phone ? { cardColumns: next } : { columns: next })
  }
  return <div data-slot="server-toolbar" className="grid gap-2 border-b px-4 py-3">
    <div className="flex flex-wrap items-center gap-2">
      {/* Phones give the search its own row; status and the two buttons share the next. */}
      <div className="relative min-w-0 flex-[1_1_14rem] max-sm:basis-full">
        <SearchIcon className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2" aria-hidden="true" />
        <Input type="search" aria-label="筛选名称、标签或地区" placeholder="搜索名称、标签或地区" className="pl-8" value={value.query} onChange={event => set({ query: event.target.value })} />
      </div>
      <NativeSelect aria-label="状态" className="w-auto max-sm:flex-1" value={value.online} onChange={event => set({ online: event.target.value as ServerView['online'] })}>
        <option value="all">全部状态</option><option value="online">在线</option><option value="offline">离线或未接入</option>
      </NativeSelect>
      <ToolbarPanel icon={ListFilterIcon} label="更多筛选" title="更多筛选" count={active.more}>
        <Field label="分组"><NativeSelect className="w-full" value={value.group} onChange={event => set({ group: event.target.value })}><option value="">全部分组</option>{groups.map(group => <option key={group} value={group}>{group}</option>)}</NativeSelect></Field>
        <Field label="地区"><NativeSelect className="w-full" value={value.region} onChange={event => set({ region: event.target.value })}><option value="">全部地区</option>{regions.map(region => <option key={region} value={region}>{region}</option>)}</NativeSelect></Field>
      </ToolbarPanel>
      <ToolbarPanel icon={Settings2Icon} label="视图" title="视图设置">
        <Field label="排序"><NativeSelect className="w-full" value={value.sort} onChange={event => set({ sort: event.target.value as ServerView['sort'] })}><option value="id">接入顺序</option><option value="name">名称</option><option value="cpu">CPU 使用率</option><option value="expiry">到期时间</option></NativeSelect></Field>
        <Field label="列表分组方式"><NativeSelect className="w-full" value={value.groupBy} onChange={event => set({ groupBy: event.target.value as ServerView['groupBy'] })}><option value="none">不分组</option><option value="group">按服务器分组</option><option value="region">按地区分组</option></NativeSelect></Field>
        <fieldset className="grid gap-2"><legend className="mb-2 text-sm font-medium">{phone ? '卡片显示' : '表格列'}</legend>
          <div className="grid grid-cols-2 gap-x-3 gap-y-2">{fields.map(([key, label]) => <label className="flex items-center gap-2 text-sm" key={key}><Checkbox checked={shown.includes(key)} onCheckedChange={checked => toggle(key, checked === true)} />{label}</label>)}</div>
        </fieldset>
        <div className="grid gap-1.5"><span className="text-sm font-medium" id="server-density-label">列表密度</span>
          <SegmentedGroup aria-labelledby="server-density-label" className="w-fit">{([['comfortable', '舒适'], ['compact', '紧凑']] as const).map(([density, label]) => <SegmentedButton key={density} aria-pressed={value.density === density} onClick={() => set({ density })}>{label}</SegmentedButton>)}</SegmentedGroup>
        </div>
        <ErrorNotice message={saved.error || action.error} retry={saved.reload} />
        <div className="flex flex-wrap items-center gap-2 border-t pt-3">
          <Button variant="outline" size="sm" disabled={!saved.ready || action.busy} onClick={() => void action.run(() => saved.save(value))}>保存当前视图</Button>
          <Button variant="outline" size="sm" disabled={!saved.ready || !saved.value} onClick={() => saved.value && onChange({ ...defaultServerView, ...saved.value })}>应用已保存视图</Button>
          <Button variant="ghost" size="sm" onClick={() => onChange(defaultServerView)}>恢复默认视图</Button>
        </div>
      </ToolbarPanel>
    </div>
    {(active.query || active.online || active.more > 0) && <div role="group" aria-label="已启用的筛选条件" className="flex flex-wrap items-center gap-2">
      {value.group && <FilterChip label={`分组：${value.group}`} onRemove={() => set({ group: '' })} />}
      {value.region && <FilterChip label={`地区：${value.region}`} onRemove={() => set({ region: '' })} />}
      <Button variant="link" size="sm" className="h-7 px-1" onClick={() => onChange(clearServerFilters(value))}>清除全部筛选</Button>
    </div>}
  </div>
}
