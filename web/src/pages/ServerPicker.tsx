import { useState } from 'react'
import type { Server } from '../types'

import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'

export default function ServerPicker({ servers, selected, onChange }: { servers: Server[]; selected: number[]; onChange: (ids: number[]) => void }) {
  const [query, setQuery] = useState('')
  const visible = servers.filter(server => `${server.name} ${server.asset_settings?.region ?? ''} ${server.asset_settings?.group_name ?? ''}`.toLowerCase().includes(query.trim().toLowerCase()))
  const all = visible.length > 0 && visible.every(server => selected.includes(server.id))
  return <fieldset className="my-4 min-w-0 rounded-xl border p-4 max-sm:p-3"><legend className="px-1.5 font-semibold">分配服务器</legend><div className="mb-3 flex flex-wrap justify-between gap-2.5 text-sm"><span>已选 {selected.length} / 共 {servers.length} 台</span><Button type="button" variant="ghost" size="sm" disabled={!visible.length} onClick={() => onChange(all ? selected.filter(id => !visible.some(server => server.id === id)) : [...new Set([...selected, ...visible.map(server => server.id)])])}>{all ? '取消当前全选' : '全选当前结果'}</Button></div><Input type="search" aria-label="搜索服务器" placeholder="搜索名称、地区或分组" value={query} onChange={event => setQuery(event.target.value)} /><div className="mt-3 grid max-h-[230px] grid-cols-1 gap-2 overflow-auto sm:grid-cols-2">{visible.map(server => <label className="bg-muted/50 m-0 flex items-center gap-2.5 rounded-lg p-2.5 wrap-anywhere" key={server.id}><Checkbox checked={selected.includes(server.id)} onCheckedChange={checked => onChange(checked === true ? [...selected, server.id] : selected.filter(id => id !== server.id))} /><span>{server.name}<small className="text-muted-foreground mt-1 block text-xs">{server.asset_settings?.group_name || '未分组'} · {server.online ? '在线' : '离线或待接入'}</small></span></label>)}{selected.filter(id => !servers.some(server => server.id === id)).map(id => <label className="bg-muted/50 m-0 flex items-center gap-2.5 rounded-lg p-2.5 wrap-anywhere" key={id}><Checkbox checked onCheckedChange={() => onChange(selected.filter(value => value !== id))} /><span>服务器 #{id}（已不存在，原选择保留）<small className="text-muted-foreground mt-1 block text-xs">明确取消此选择后才能保存。</small></span></label>)}{!visible.length && <p className="text-muted-foreground text-xs">没有匹配的服务器。</p>}</div></fieldset>
}
