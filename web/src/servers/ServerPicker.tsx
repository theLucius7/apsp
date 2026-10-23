import { useState } from 'react'
import type { Server } from '../types'
import '../monitoring/monitoring.css'

export default function ServerPicker({ servers, selected, onChange }: { servers: Server[]; selected: number[]; onChange: (ids: number[]) => void }) {
  const [query, setQuery] = useState('')
  const visible = servers.filter(server => `${server.name} ${server.asset_settings?.region ?? ''} ${server.asset_settings?.group_name ?? ''}`.toLowerCase().includes(query.trim().toLowerCase()))
  const all = visible.length > 0 && visible.every(server => selected.includes(server.id))
  return <fieldset className="monitoring-picker"><legend>分配服务器</legend><div className="monitoring-picker-heading"><span>已选 {selected.length} / 共 {servers.length} 台</span><button type="button" className="text-button" disabled={!visible.length} onClick={() => onChange(all ? selected.filter(id => !visible.some(server => server.id === id)) : [...new Set([...selected, ...visible.map(server => server.id)])])}>{all ? '取消当前全选' : '全选当前结果'}</button></div><input type="search" aria-label="搜索服务器" placeholder="搜索名称、地区或分组" value={query} onChange={event => setQuery(event.target.value)} /><div className="monitoring-picker-list">{visible.map(server => <label key={server.id}><input type="checkbox" checked={selected.includes(server.id)} onChange={event => onChange(event.target.checked ? [...selected, server.id] : selected.filter(id => id !== server.id))} /><span>{server.name}<small>{server.asset_settings?.group_name || '未分组'} · {server.online ? '在线' : '离线或待接入'}</small></span></label>)}{selected.filter(id => !servers.some(server => server.id === id)).map(id => <label key={id}><input type="checkbox" checked onChange={() => onChange(selected.filter(value => value !== id))} /><span>服务器 #{id}（已不存在，原选择保留）<small>明确取消此选择后才能保存。</small></span></label>)}{!visible.length && <p className="subtle">没有匹配的服务器。</p>}</div></fieldset>
}
