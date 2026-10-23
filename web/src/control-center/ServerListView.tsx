import { usePreference } from './preferences'
import { useAction } from '../hooks'
import { ErrorNotice } from '../components'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Panel } from '@/components/panel'
export type ServerView = { query: string; online: 'all' | 'online' | 'offline'; group: string; groupBy: 'none' | 'group' | 'region'; sort: 'id' | 'name' | 'cpu' | 'expiry'; columns: string[]; density: 'comfortable' | 'compact' }
export const defaultServerView: ServerView = { query: '', online: 'all', group: '', groupBy: 'none', sort: 'id', columns: ['cpu', 'memory', 'cost', 'traffic', 'version'], density: 'comfortable' }
export default function ServerListView({ value, onChange, groups }: { value: ServerView; onChange: (value: ServerView) => void; groups: string[] }) {
  const saved = usePreference<ServerView>('view:servers')
  const action = useAction()
  const columns: [string, string][] = [['cpu', '处理器'], ['memory', '内存'], ['cost', '成本到期'], ['traffic', '流量'], ['version', '版本']]
  return <Panel><div className="p-4"><div className="grid gap-4 sm:grid-cols-2">
    <label data-slot="field" className="mb-4 grid min-w-0 gap-1.5 last:mb-0"><span className="text-sm leading-snug font-medium">筛选名称、标签或地区</span><Input type="search" value={value.query} onChange={event => onChange({ ...value, query: event.target.value })} /></label>
    <label data-slot="field" className="mb-4 grid min-w-0 gap-1.5 last:mb-0"><span className="text-sm leading-snug font-medium">状态</span><NativeSelect className="w-full" value={value.online} onChange={event => onChange({ ...value, online: event.target.value as ServerView['online'] })}><option value="all">全部</option><option value="online">在线</option><option value="offline">离线或未接入</option></NativeSelect></label>
    <label data-slot="field" className="mb-4 grid min-w-0 gap-1.5 last:mb-0"><span className="text-sm leading-snug font-medium">分组</span><NativeSelect className="w-full" value={value.group} onChange={event => onChange({ ...value, group: event.target.value })}><option value="">全部分组</option>{groups.map(group => <option key={group} value={group}>{group || '未分组'}</option>)}</NativeSelect></label>
    <label data-slot="field" className="mb-4 grid min-w-0 gap-1.5 last:mb-0"><span className="text-sm leading-snug font-medium">排序</span><NativeSelect className="w-full" value={value.sort} onChange={event => onChange({ ...value, sort: event.target.value as ServerView['sort'] })}><option value="id">接入顺序</option><option value="name">名称</option><option value="cpu">CPU 使用率</option><option value="expiry">到期时间</option></NativeSelect></label>
    <label data-slot="field" className="mb-4 grid min-w-0 gap-1.5 last:mb-0"><span className="text-sm leading-snug font-medium">列表分组方式</span><NativeSelect className="w-full" value={value.groupBy} onChange={event => onChange({ ...value, groupBy: event.target.value as ServerView['groupBy'] })}><option value="none">不分组</option><option value="group">按服务器分组</option><option value="region">按地区分组</option></NativeSelect></label>
  </div><div className="flex flex-wrap items-center gap-2.5">{columns.map(([key, label]) => <label className="flex items-center gap-2 text-sm" key={key}><Checkbox checked={value.columns.includes(key)} onCheckedChange={checked => onChange({ ...value, columns: checked === true ? [...value.columns, key] : value.columns.filter(item => item !== key) })} />{label}</label>)}<label className="flex items-center gap-2 text-sm"><Checkbox checked={value.density === 'compact'} onCheckedChange={checked => onChange({ ...value, density: checked === true ? 'compact' : 'comfortable' })} />紧凑列表</label></div>
    <ErrorNotice message={saved.error || action.error} retry={saved.reload} /><div className="flex flex-wrap items-center gap-2.5"><Button variant="outline" disabled={!saved.ready || action.busy} onClick={() => void action.run(() => saved.save(value))}>保存当前视图</Button><Button variant="outline" disabled={!saved.ready || !saved.value} onClick={() => saved.value && onChange({ ...defaultServerView, ...saved.value })}>应用已保存视图</Button><Button variant="outline" onClick={() => onChange(defaultServerView)}>恢复默认视图</Button></div>
  </div></Panel>
}
