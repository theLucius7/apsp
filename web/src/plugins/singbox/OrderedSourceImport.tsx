import { useEffect, useRef, useState } from 'react'
import { errorMessage } from '../../api'
import { Badge, ErrorNotice, Field, Modal } from '../../components'
import { assignmentRequestId } from './groupTypes'
import { commitOrderedPreview, createOrderedPreview, discardOrderedPreview, emptySourceDraft, previewCommit, previewInput, SourceFileReader, sourceMetadataError } from './sourceRequests'
import type { SourceDraft, SourceSnapshot } from './sourceRequests'
import { orderedPreviewError, sourceFormatText } from './orderedSourceTypes'
import type { OrderedPreview, OrderedPreviewNode, SourceReceipt } from './orderedSourceTypes'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'

const PAGE = 50
const endpoint = (node: OrderedPreviewNode) => node.server && node.server_port !== null ? `${node.server.includes(':') ? `[${node.server}]` : node.server}:${node.server_port}` : '端点未知'

// Fetch and parse first, then save the source with only the chosen nodes in the catalog.
export default function OrderedSourceImport({ snapshot, onClose, onSaved }: { snapshot: SourceSnapshot; onClose: () => void; onSaved: (receipt: SourceReceipt) => void }) {
  const [draft, setDraft] = useState<SourceDraft>(emptySourceDraft)
  const [preview, setPreview] = useState<OrderedPreview | null>(null)
  const [selected, setSelected] = useState<string[]>([])
  const [expired, setExpired] = useState(false)
  const [filter, setFilter] = useState('')
  const [page, setPage] = useState(0)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [fileName, setFileName] = useState('')
  const [fileBusy, setFileBusy] = useState(false)
  const [fileKey, setFileKey] = useState(0)
  const reader = useRef(new SourceFileReader())
  const pending = useRef(false), active = useRef(true), previewId = useRef<string | null>(null)
  // A retried commit reuses its request id, so a lost response never saves twice.
  const request = useRef<{ signature: string; id: string } | null>(null)
  useEffect(() => {
    active.current = true
    const files = reader.current
    return () => {
      active.current = false; files.invalidate()
      const id = previewId.current; previewId.current = null
      if (id) void discardOrderedPreview(id).catch(() => { /* Expiry also removes abandoned previews. */ })
    }
  }, [])
  useEffect(() => {
    setExpired(false)
    if (!preview) return
    const timer = window.setTimeout(() => setExpired(true), Math.max(0, Math.min(preview.expires_at * 1000 - Date.now(), 2147483647)))
    return () => window.clearTimeout(timer)
  }, [preview])
  const writeError = sourceMetadataError(snapshot) || (preview && expired ? '导入预览已过期，请重新填写；当前选择已保留。' : '')
  const set = (key: keyof SourceDraft, value: string) => { if (busy) return; setError(''); setDraft(current => ({ ...current, [key]: value })) }
  const switchKind = (kind: 'url' | 'inline') => {
    if (busy) return
    reader.current.invalidate(); setFileBusy(false); setFileName(''); setFileKey(value => value + 1); setError('')
    setDraft(current => ({ ...current, kind, url: '', content: '', authorization: '', cookie: '', apiKey: '' }))
  }
  const selectFile = (file?: File) => {
    if (!file || busy) return
    setFileBusy(true); setError('')
    void reader.current.read(file).then(content => {
      if (content === null || !active.current) return
      setDraft(current => ({ ...current, content })); setFileName(file.name); setFileBusy(false)
    }).catch(cause => { if (active.current) { setError(errorMessage(cause)); setFileBusy(false) } })
  }
  const restart = () => {
    if (busy) return
    const id = previewId.current; previewId.current = null
    if (id && !sourceMetadataError(snapshot)) void discardOrderedPreview(id).catch(() => { /* Expiry also removes abandoned previews. */ })
    request.current = null; setPreview(null); setSelected([]); setFilter(''); setPage(0); setError('')
  }
  const submit = async () => {
    if (pending.current || fileBusy || writeError) return
    pending.current = true; setBusy(true); setError('')
    try {
      if (!preview) {
        const result = await createOrderedPreview(previewInput(draft))
        if (!active.current) { void discardOrderedPreview(result.id).catch(() => { /* Expiry also removes abandoned previews. */ }); return }
        previewId.current = result.id
        // The panel keeps the fetched body; secrets leave page memory now.
        reader.current.invalidate(); setFileName(''); setFileKey(value => value + 1)
        setDraft(current => ({ ...current, url: '', content: '', authorization: '', cookie: '', apiKey: '' }))
        setPreview(result); setSelected(result.nodes.filter(node => node.selectable).map(node => node.key)); setPage(0)
      } else {
        const unsigned = previewCommit(preview, draft, selected, assignmentRequestId())
        const signature = JSON.stringify({ ...unsigned, request_id: null, preview: preview.id })
        if (request.current?.signature !== signature) request.current = { signature, id: unsigned.request_id }
        const receipt = await commitOrderedPreview(preview, { ...unsigned, request_id: request.current.id })
        previewId.current = null
        if (active.current) onSaved(receipt)
      }
    } catch (cause) { if (active.current) setError(errorMessage(cause)) }
    finally { pending.current = false; if (active.current) setBusy(false) }
  }
  const needle = filter.trim().toLocaleLowerCase()
  const visible = preview?.nodes.filter(node => !needle || `${node.name} ${node.protocol ?? ''} ${node.server ?? ''}`.toLocaleLowerCase().includes(needle)) ?? []
  const pages = Math.max(1, Math.ceil(visible.length / PAGE)), current = Math.min(page, pages - 1), rows = visible.slice(current * PAGE, (current + 1) * PAGE)
  const choose = (keys: string[], chosen: boolean) => { if (busy) return; setError(''); setSelected(previous => chosen ? [...new Set([...previous, ...keys])] : previous.filter(key => !keys.includes(key))) }
  const selectionError = preview ? orderedPreviewError(preview, selected) : ''
  return <Modal wide  title={preview ? '选择加入节点库的节点' : '导入订阅来源'} onClose={onClose} busy={busy}><form onSubmit={event => { event.preventDefault(); void submit() }}><div data-slot="modal-body" className="p-4">
    <div className="mb-4 flex flex-wrap justify-between gap-3"><Badge tone={!preview ? 'good' : 'neutral'}>1 · 获取并解析</Badge><Badge tone={preview ? 'good' : 'neutral'}>2 · 选择节点并保存</Badge></div>
    <ErrorNotice message={writeError || error} />
    <fieldset disabled={busy}>
      <Field label="来源名称"><Input name="import_name" required maxLength={128} value={draft.name} onChange={event => set('name', event.target.value)} autoComplete="off" /></Field>
      {!preview ? <>
        <Field label="输入方式"><NativeSelect className="w-full" aria-label="导入输入方式" value={draft.kind} onChange={event => switchKind(event.target.value as 'url' | 'inline')}><option value="url">HTTPS 订阅地址</option><option value="inline">粘贴内容或上传文件</option></NativeSelect></Field>
        {draft.kind === 'url' ? <>
          <Field label="HTTPS 订阅地址" hint="由面板抓取；完整地址不会通过读取接口返回。"><Input type="url" name="import_url" required value={draft.url} onChange={event => set('url', event.target.value)} autoComplete="off" spellCheck={false} /></Field>
          <details className="py-1"><summary className="cursor-pointer pt-1.5 pb-3 text-sm">认证头与请求标识（可选）</summary><p className="text-muted-foreground mt-3 text-xs leading-relaxed">认证头仅支持以下三项，合计不超过 8 KiB；获取预览后立即清空，不会回填。</p>{(['authorization', 'cookie', 'apiKey'] as const).map((key, index) => <Field key={key} label={['Authorization', 'Cookie', 'X-API-Key'][index]}><Input type="password" name={`import_${key}`} value={draft[key]} onChange={event => set(key, event.target.value)} autoComplete="new-password" spellCheck={false} /></Field>)}<Field label="请求标识（User-Agent）" hint="留空则不发送。"><Input name="import_user_agent" maxLength={256} value={draft.userAgent} onChange={event => set('userAgent', event.target.value)} autoComplete="off" spellCheck={false} /></Field></details>
        </> : <>
          <Field label="订阅内容" hint="支持 URI、Base64 URI、sing-box JSON 和 Clash YAML；最多 2 MiB。"><Textarea className="min-h-36 resize-y font-mono" name="import_content" rows={8} value={draft.content} onChange={event => { set('content', event.target.value); setFileName('') }} autoComplete="off" spellCheck={false} /></Field>
          <Field label="订阅文件" hint="读取 UTF-8 文件，仅在当前浏览器内存中保存内容。"><Input key={fileKey} type="file" name="import_file" onChange={event => selectFile(event.target.files?.[0])} /></Field>
          {fileBusy && <p role="status">正在读取文件…</p>}{fileName && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">已读取：{fileName}</p>}
        </>}
      </> : <>
        <div className="mb-4 flex flex-wrap justify-between gap-3"><span>{sourceFormatText[preview.format] ?? preview.format} · 可加入 {preview.nodes.filter(node => node.selectable).length} · 不支持 {preview.unsupported_count}</span><small className="text-muted-foreground">预览保留至 {new Date(preview.expires_at * 1000).toLocaleTimeString('zh-CN', { hour12: false })}</small></div>
        {preview.warnings.map(warning => <p key={warning.code} className="text-muted-foreground mt-3 text-xs leading-relaxed">{warning.message}</p>)}
        <Field label="筛选预览节点"><Input value={filter} onChange={event => { setFilter(event.target.value); setPage(0) }} placeholder="搜索名称、协议或地址" /></Field>
        <div className="flex flex-wrap items-center justify-end gap-2"><Button type="button" variant="ghost" size="sm" onClick={() => choose(visible.filter(node => node.selectable).map(node => node.key), true)}>选择筛选结果</Button><Button type="button" variant="ghost" size="sm" onClick={() => choose(visible.map(node => node.key), false)}>取消选择筛选结果</Button><Button type="button" variant="ghost" size="sm" onClick={restart}>重新填写来源</Button></div>
        <div className="max-w-full min-w-0 max-h-[420px] overflow-auto"><Table><TableHeader><TableRow><TableHead>加入</TableHead><TableHead>节点</TableHead><TableHead>协议 / 地址</TableHead><TableHead>状态</TableHead></TableRow></TableHeader><TableBody>{rows.map(node => <TableRow key={node.key}><TableCell className="max-w-[280px] wrap-anywhere"><Checkbox aria-label={`加入 ${node.name}`} disabled={!node.selectable} checked={selected.includes(node.key)} onCheckedChange={checked => choose([node.key], checked === true)} /></TableCell><TableCell className="max-w-[280px] wrap-anywhere">{node.name}</TableCell><TableCell className="max-w-[280px] wrap-anywhere">{node.protocol ?? '协议未知'}<small>{endpoint(node)}</small></TableCell><TableCell className="max-w-[280px] wrap-anywhere">{node.selectable ? <Badge tone="good">可加入</Badge> : <small>{node.parse_status !== 'supported' ? node.unsupported_reasons.map(reason => reason.message).join('；') || '不支持解析' : node.identity_state === 'ambiguous' ? '节点身份不唯一' : '尚未建立可校验的节点身份'}</small>}</TableCell></TableRow>)}</TableBody></Table></div>
        <div className="text-muted-foreground my-4 flex flex-wrap items-center justify-center gap-3 text-xs"><Button type="button" variant="outline" disabled={current === 0} onClick={() => setPage(current - 1)}>上一页</Button><span>第 {current + 1} / {pages} 页 · 已选 {selected.length} 个</span><Button type="button" variant="outline" disabled={current >= pages - 1} onClick={() => setPage(current + 1)}>下一页</Button></div>
      </>}
      {draft.kind === 'url' && <><label className="mb-4 flex items-center gap-2.5 text-sm"><Checkbox name="import_auto_refresh" checked={draft.autoRefresh === 'on'} onCheckedChange={checked => set('autoRefresh', checked === true ? 'on' : 'off')} />按周期自动刷新</label>
        <Field label="自动刷新周期（秒）" hint="最少 300 秒，最多 2592000 秒；默认每天一次。"><Input type="number" name="import_interval" required min={300} max={2592000} step={1} value={draft.interval} onChange={event => set('interval', event.target.value)} /></Field></>}
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">保存时按预览时取得的内容创建来源；只有所选节点加入节点库，其余节点仍可在来源详情中加入，也可直接用于有序链路。</p>
    </fieldset>
  </div><footer><Button type="button" variant="outline" onClick={onClose} disabled={busy}>取消</Button><Button disabled={busy || fileBusy || Boolean(writeError) || Boolean(selectionError)}>{busy ? '正在处理…' : preview ? `保存来源并加入节点库（${selected.length}）` : '获取并预览'}</Button></footer></form></Modal>
}
