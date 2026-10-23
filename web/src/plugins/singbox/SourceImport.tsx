import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from '../../api'
import { Badge, Field, FormDialog } from '../../components'
import type { SubscriptionSource } from './sourceTypes'
import { sourceReason, sourcePreviewError, validSourcePreview } from './sourceTypes'

import type { SourcePreview as Preview } from './sourceTypes'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'

const root = '/api/plugins/sing-box/subscription-source-previews'

export default function SourceImport({ writeError, onClose, onSaved }: { writeError: () => string; onClose: () => void; onSaved: (source: SubscriptionSource) => void }) {
  const [kind, setKind] = useState('url'), [name, setName] = useState(''), [url, setUrl] = useState(''), [authorization, setAuthorization] = useState(''), [content, setContent] = useState('')
  const [agent, setAgent] = useState('Apsp-subscription-import/1'), [auto, setAuto] = useState(true), [interval, setInterval] = useState(1440)
  const [preview, setPreview] = useState<Preview | null>(null), [selected, setSelected] = useState<string[]>([])
  const [expired, setExpired] = useState(false)
  const [query, setQuery] = useState(''), [page, setPage] = useState(1), [error, setError] = useState(''), [busy, setBusy] = useState(false)
  const pending = useRef(false), controller = useRef<AbortController | null>(null), active = useRef(true), previewId = useRef<string | null>(null)
  const discard = (explicit = false) => { const id = previewId.current; previewId.current = null; if (explicit && !writeError() && id) void api(`${root}/${id}`, 'DELETE').catch(() => { /* Expiration also removes abandoned previews. */ }) }
  useEffect(() => { active.current = true; return () => { active.current = false; controller.current?.abort(); previewId.current = null } }, [])
  useEffect(() => { setExpired(false); if (!preview) return; const remaining = preview.expires_at * 1000 - Date.now(); const timer = window.setTimeout(() => setExpired(true), Math.max(0, Math.min(remaining, 2147483647))); return () => window.clearTimeout(timer) }, [preview])
  const submit = async () => {
    if (pending.current) return
    const stale = writeError(); if (stale) { setError(stale); return }
    if (!name.trim() || Array.from(name.trim()).length > 128 || /[\u0000-\u001f\u007f]/.test(name) || !['url', 'inline'].includes(kind) || !Number.isSafeInteger(interval) || interval < 5 || interval > 43200) { setError('来源名称或刷新间隔不符合要求，请保留草稿并修正。'); return }
    if (!preview && (kind === 'inline' ? !content.trim() : !/^https:\/\//i.test(url.trim()) || url.length > 8192)) { setError('请填写 HTTPS 来源地址或非空配置内容。'); return }
    pending.current = true; setBusy(true); setError('')
    const request = new AbortController(); controller.current = request
    try {
      if (preview) {
        const previewError = sourcePreviewError(preview, selected); if (previewError) throw new Error(previewError)
        const source = await api<SubscriptionSource>(`${root}/${preview.id}/commit`, 'POST', { name: name.trim(), selected, auto_refresh: kind === 'url' && auto, refresh_interval_seconds: interval * 60 }, request.signal)
        previewId.current = null
        if (active.current) onSaved(source)
      } else {
        if (new TextEncoder().encode(content).length > 2 * 1024 * 1024) throw new Error('配置内容不能超过 2 MiB。')
        const result = await api<unknown>(root, 'POST', { name: name.trim(), kind, ...(kind === 'url' ? { url: url.trim(), ...(authorization ? { authorization } : {}), user_agent: agent } : { content }) }, request.signal)
        if (!validSourcePreview(result)) throw new Error('面板返回的导入预览格式不完整，请重新解析。')
        previewId.current = result.id
        if (!active.current) { discard(); return }
        setPreview(result); setSelected(result.nodes.filter(node => node.supported).map(node => node.key)); setContent(''); setUrl(''); setAuthorization(''); setPage(1)
      }
    } catch (cause) { if (active.current) setError(errorMessage(cause)) }
    finally { pending.current = false; if (active.current) setBusy(false) }
  }
  const upload = async (file?: File) => {
    if (!file) return
    if (file.size > 2 * 1024 * 1024) { setError('配置文件不能超过 2 MiB。'); return }
    try { const text = await file.text(); if (active.current) { setContent(text); setError('') } } catch (cause) { if (active.current) setError(errorMessage(cause)) }
  }
  const visible = preview?.nodes.filter(node => `${node.name} ${node.protocol ?? ''} ${node.server ?? ''}`.toLocaleLowerCase().includes(query.toLocaleLowerCase())) ?? []
  const pages = Math.max(1, Math.ceil(visible.length / 50)), currentPage = Math.min(page, pages), rows = visible.slice((currentPage - 1) * 50, currentPage * 50)
  return <FormDialog title={preview ? '选择导入节点' : '导入外部节点'} wide busy={busy} onClose={onClose} onSubmit={() => void submit()} error={writeError() || (preview && expired ? '导入预览已过期，请重新解析；当前选择已保留。' : '') || error} submitDisabled={Boolean(writeError()) || Boolean(preview && sourcePreviewError(preview, selected))} submitLabel={preview ? `加入节点库（${selected.length}）` : '解析并预览'}>
    <div className="mb-4 flex flex-wrap justify-between gap-3"><Badge tone={!preview ? 'good' : 'neutral'}>1 · 填写来源</Badge><Badge tone={preview ? 'good' : 'neutral'}>2 · 确认节点</Badge></div>
    <Field label="来源名称"><Input required maxLength={128} value={name} onChange={event => setName(event.target.value)} autoComplete="off" /></Field>
    {!preview ? <><Field label="来源类型"><NativeSelect className="w-full" value={kind} onChange={event => { setKind(event.target.value); setUrl(''); setAuthorization(''); setContent('') }}><option value="url">HTTPS 订阅地址</option><option value="inline">粘贴或上传配置</option></NativeSelect></Field>
      {kind === 'url' ? <><Field label="HTTPS 订阅地址"><Input type="password" required autoComplete="new-password" spellCheck={false} maxLength={8192} value={url} onChange={event => setUrl(event.target.value)} placeholder="https://…" /></Field><details><summary className="cursor-pointer text-sm font-medium">请求选项</summary><Field label="获取认证（可选）"><Input type="password" autoComplete="new-password" maxLength={8192} value={authorization} onChange={event => setAuthorization(event.target.value)} /></Field><Field label="请求标识"><Input required maxLength={256} value={agent} onChange={event => setAgent(event.target.value)} /></Field></details></> : <><Field label="上传配置文件"><Input type="file" accept=".txt,.json,.yaml,.yml,text/plain,application/json" onChange={event => void upload(event.target.files?.[0])} /></Field><Field label="配置内容" hint="分享链接、Base64 订阅、sing-box JSON 或 Clash 节点配置，最大 2 MiB。"><Textarea required rows={8} autoComplete="off" spellCheck={false} value={content} onChange={event => setContent(event.target.value)} /></Field></>}
    </> : <><div className="mb-4 flex flex-wrap justify-between gap-3"><span>可导入 {preview.supported_count} · 不支持 {preview.unsupported_count}</span><small className="text-muted-foreground">预览保留至 {new Date(preview.expires_at * 1000).toLocaleTimeString('zh-CN', { hour12: false })}</small></div><Field label="筛选预览节点"><Input value={query} onChange={event => { setQuery(event.target.value); setPage(1) }} placeholder="搜索名称、协议或地址" /></Field><div className="flex flex-wrap items-center justify-end gap-2"><Button type="button" variant="ghost" size="sm" onClick={() => setSelected([...new Set([...selected, ...visible.filter(node => node.supported).map(node => node.key)])])}>选择筛选结果</Button><Button type="button" variant="ghost" size="sm" onClick={() => setSelected([])}>清空选择</Button><Button type="button" variant="ghost" size="sm" disabled={busy || Boolean(writeError())} onClick={() => { if (writeError()) return; discard(true); setPreview(null); setSelected([]); setQuery(''); setError('') }}>重新填写</Button></div>
      <div className="max-w-full min-w-0 max-h-[420px] overflow-auto"><Table><TableHeader><TableRow><TableHead>导入</TableHead><TableHead>节点</TableHead><TableHead>协议 / 地址</TableHead><TableHead>状态</TableHead></TableRow></TableHeader><TableBody>{rows.map(node => <TableRow key={node.key}><TableCell className="max-w-[280px] wrap-anywhere"><Checkbox disabled={!node.supported} aria-label={`导入 ${node.name}`} checked={selected.includes(node.key)} onCheckedChange={checked => { if (writeError() || !node.supported) return; setSelected(checked === true ? [...new Set([...selected, node.key])] : selected.filter(key => key !== node.key)) }} /></TableCell><TableCell className="max-w-[280px] wrap-anywhere">{node.name}</TableCell><TableCell className="max-w-[280px] wrap-anywhere">{node.protocol ?? '—'}<small>{node.server ?? '—'}{node.port ? `:${node.port}` : ''}</small></TableCell><TableCell className="max-w-[280px] wrap-anywhere">{node.supported ? <Badge tone="good">支持</Badge> : sourceReason(node.reason)}</TableCell></TableRow>)}</TableBody></Table></div><div className="flex flex-wrap items-center justify-between gap-3.5 border-t px-5 py-3 text-sm max-md:gap-2.5 max-md:p-3"><span>第 {currentPage} / {pages} 页</span><div className="flex flex-wrap items-center justify-end gap-2"><Button type="button" variant="ghost" size="sm" disabled={currentPage <= 1} onClick={() => setPage(currentPage - 1)}>上一页</Button><Button type="button" variant="ghost" size="sm" disabled={currentPage >= pages} onClick={() => setPage(currentPage + 1)}>下一页</Button></div></div>
    </>}
    {kind === 'url' && <><label className="mb-4 flex items-center gap-2.5 text-sm"><Checkbox checked={auto} onCheckedChange={checked => setAuto(checked === true)} />自动更新来源</label><Field label="刷新间隔（分钟）"><Input type="number" min={5} max={43200} step={1} required value={interval} onChange={event => setInterval(Number(event.target.value))} /></Field></>}
  </FormDialog>
}
