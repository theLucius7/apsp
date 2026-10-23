import { useCallback, useEffect, useRef, useState } from 'react'
import { api, errorMessage } from '../../api'
import { Badge, ErrorNotice, Field, Loading, Modal } from '../../components'
import { bytes, time } from '../../format'
import type { ProxyUser } from '../../types'
import { statusText } from './groupTypes'
import { protocolNames } from './ProtocolFields'

import { Button } from '@/components/ui/button'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'
import { ArrowDownIcon, CopyIcon, LockIcon, RefreshCwIcon } from 'lucide-react'
import { Notice } from '@/components/panel'

export type SubscriptionFormat = 'singbox' | 'links'
type Subscription = {
  format: SubscriptionFormat; status: 'ready' | 'empty' | 'blocked' | 'format_unavailable'; message: string
  subscription_url: string; available_formats: string[]; granted_nodes: number; eligible_nodes: number
  ready_nodes: { kind?: 'managed' | 'external'; id: number; name: string; protocol: string; source_last_error?: string | null }[]
  managed_nodes?: number; external_nodes?: number; external_granted_nodes?: number
  content: string | null; filename: string; content_type: string
  entitlement: { status: string; allowed: boolean; monthly_bytes: string | null; used_bytes: string; expires_at: number | null }
}
type Action = 'address' | 'preview' | 'copy' | 'download'
const formatNames = { singbox: 'sing-box 完整配置', links: '分享链接（仅 Reality）' }
const statusNames = { ready: '可以获取', empty: '等待可用节点', blocked: '套餐受限', format_unavailable: '格式不可用' }

function address(subscription: Subscription): string {
  try {
    const url = new URL(subscription.subscription_url)
    if (!['https:', 'http:'].includes(url.protocol)) return ''
    url.searchParams.set('format', subscription.format)
    return url.toString()
  } catch { return '' }
}

async function copy(text: string) {
  if (navigator.clipboard?.writeText) return navigator.clipboard.writeText(text)
  const input = document.createElement('textarea')
  input.value = text; input.style.position = 'fixed'; input.style.opacity = '0'; document.body.append(input)
  try { input.select(); if (!document.execCommand('copy')) throw new Error('clipboard unavailable') }
  finally { input.remove() }
}

export default function SubscriptionDialog({ user, format, onFormatChange, onClose, onReset }: {
  user: ProxyUser; format: SubscriptionFormat; onFormatChange: (format: SubscriptionFormat) => void; onClose: () => void; onReset: () => void
}) {
  const [data, setData] = useState<Subscription | null>(null)
  const [loading, setLoading] = useState(true)
  const [working, setWorking] = useState<Action | null>(null)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [preview, setPreview] = useState(false)
  const sequence = useRef(0), controller = useRef<AbortController | null>(null), alive = useRef(true), locked = useRef(false)
  const load = useCallback(async (): Promise<Subscription | null> => {
    const revision = ++sequence.current
    controller.current?.abort()
    const request = new AbortController(); controller.current = request
    setLoading(true); setData(null); setError(''); setNotice('')
    try {
      const value = await api<Subscription>(`/api/plugins/sing-box/users/${user.id}/subscription?format=${format}`, 'GET', undefined, request.signal)
      if (!alive.current || request.signal.aborted || revision !== sequence.current) return null
      if (value.format !== format) throw new Error('返回的订阅格式与请求不符，请刷新重试。')
      setData(value)
      return value
    } catch (failure) {
      if (alive.current && !request.signal.aborted && revision === sequence.current) { setData(null); setError(errorMessage(failure)) }
      return null
    } finally { if (alive.current && revision === sequence.current) setLoading(false) }
  }, [user.id, format])
  useEffect(() => {
    alive.current = true
    void load()
    return () => { alive.current = false; ++sequence.current; controller.current?.abort() }
  }, [load])
  const current = data?.format === format ? data : null
  useEffect(() => {
    if (data?.external_granted_nodes && format !== 'singbox') { setPreview(false); onFormatChange('singbox') }
  }, [data?.external_granted_nodes, format, onFormatChange])
  const ready = current?.status === 'ready' && current.content !== null
  const busy = loading || working !== null
  const link = current ? address(current) : ''
  const perform = async (action: Action) => {
    if (locked.current) return
    locked.current = true; setWorking(action)
    try {
      const value = await load()
      if (!value) return
      if (action === 'address') {
        const next = address(value)
        if (value.status === 'format_unavailable' || !next) return
        try { await copy(next); if (alive.current) setNotice('订阅地址已复制。') }
        catch { if (alive.current) setError('复制失败，请选中订阅地址手动复制。') }
      } else if (value.status === 'ready' && value.content !== null) {
        if (action === 'preview') setPreview(true)
        if (action === 'copy') {
          try { await copy(value.content); if (alive.current) setNotice('配置内容已复制。') }
          catch { if (alive.current) { setPreview(true); setError('复制失败，请在配置预览中手动选择并复制。') } }
        }
        if (action === 'download') {
          const url = URL.createObjectURL(new Blob([value.content], { type: format === 'singbox' ? 'application/json;charset=utf-8' : 'text/plain;charset=utf-8' }))
          const anchor = document.createElement('a')
          anchor.href = url; anchor.download = format === 'singbox' ? 'apsp-subscription.json' : 'apsp-subscription.txt'
          try { document.body.append(anchor); anchor.click(); setNotice('已请求浏览器保存订阅文件。') }
          finally { anchor.remove(); window.setTimeout(() => URL.revokeObjectURL(url), 1000) }
        }
      }
    } catch { if (alive.current) setError('获取订阅未完成，请刷新后重试。') }
    finally { locked.current = false; if (alive.current) setWorking(null) }
  }
  return <Modal slot="subscription-dialog" title={`${user.name} 的订阅`} onClose={onClose} wide className="sm:max-w-[47.5rem]">
    <div data-slot="modal-body" className="p-4">
      <div className="mb-5 flex items-end gap-4 max-sm:gap-2.5 max-[360px]:flex-col max-[360px]:items-stretch"><Field className="m-0 min-w-0 flex-1" label="订阅格式"><NativeSelect className="w-full" value={format} disabled={busy} onChange={event => { setPreview(false); setData(null); onFormatChange(event.target.value as SubscriptionFormat) }}>{Object.entries(formatNames).map(([value, name]) => <option key={value} value={value} disabled={value === 'links' && Boolean(current?.external_granted_nodes)}>{name}</option>)}</NativeSelect></Field><Button variant="outline" className="shrink-0" disabled={busy} onClick={() => void load()}><RefreshCwIcon aria-hidden="true" />刷新状态</Button></div>
      <ErrorNotice message={error} retry={busy ? undefined : () => void load()} />
      {notice && <Notice tone="success" role="status">{notice}</Notice>}
      {loading && <Loading />}
      {current && <>
        <div className="mb-4 flex items-start gap-2.5 max-sm:flex-wrap"><Badge tone={current.status === 'ready' ? 'good' : current.status === 'blocked' ? 'bad' : 'warm'}>{statusNames[current.status]}</Badge><p className="text-muted-foreground text-sm leading-relaxed">{current.message}</p></div>
        <dl className="bg-muted/50 m-0 grid gap-x-5 gap-y-4 rounded-lg p-4 sm:grid-cols-2"><div><dt className="text-muted-foreground text-xs">已授权 / 符合套餐条件 / 可用节点</dt><dd className="mt-1.5 text-sm wrap-anywhere">{current.granted_nodes} / {current.eligible_nodes} / {current.ready_nodes.length}</dd></div><div><dt className="text-muted-foreground text-xs">套餐状态</dt><dd className="mt-1.5 text-sm wrap-anywhere">{statusText[current.entitlement.status as keyof typeof statusText] ?? '未知状态'}</dd></div><div><dt className="text-muted-foreground text-xs">本期已用 / 每月额度</dt><dd className="mt-1.5 text-sm wrap-anywhere">{bytes(current.entitlement.used_bytes)} / {current.entitlement.monthly_bytes === null ? '不限量' : bytes(current.entitlement.monthly_bytes)}</dd></div><div><dt className="text-muted-foreground text-xs">到期时间</dt><dd className="mt-1.5 text-sm wrap-anywhere">{current.entitlement.expires_at === null ? '不限期' : time(current.entitlement.expires_at)}</dd></div></dl>
        <div data-slot="subscription-nodes" className="mt-5"><h3 className="text-sm font-semibold">{current.external_granted_nodes ? '当前可订阅节点' : '已应用且健康的节点'}</h3>{current.ready_nodes.length ? <ul className="mt-2.5 max-h-44 list-none overflow-auto p-0">{current.ready_nodes.map(node => <li className="flex items-center justify-between gap-3 border-b py-2.5 text-sm max-sm:items-start" key={`${node.kind ?? 'managed'}-${node.id}`}><span className="min-w-0 wrap-anywhere">{node.name}</span><small className="text-muted-foreground shrink-0 text-xs">{node.kind === 'external' ? '外部 · ' : ''}{protocolNames[node.protocol] ?? node.protocol}{node.source_last_error ? ' · 保留上次成功版本' : ''}</small></li>)}</ul> : <p className="text-muted-foreground mt-3 text-xs leading-relaxed">暂无可用节点；等待授权、套餐条件或节点状态更新后刷新。</p>}</div>
        {!!current.external_granted_nodes && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">受管 {Number.isSafeInteger(current.managed_nodes) && current.managed_nodes! >= 0 ? current.managed_nodes : '—'} 个 · 外部 {Number.isSafeInteger(current.external_nodes) && current.external_nodes! >= 0 ? current.external_nodes : '—'} 个。外部用量未知，由提供方计量；取消分配或套餐受限会停止后续订阅获取，已下载的外部凭据仍由提供方控制。</p>}
        <p className="text-muted-foreground mt-3 text-xs leading-relaxed">当前可用格式：{current.available_formats.map(value => formatNames[value as SubscriptionFormat]).filter(Boolean).join('、') || '暂无'}。刚授权的节点需要等待设备成功应用。</p>
      </>}
      <section className="mt-5 border-t pt-5" aria-label="订阅地址"><h3 className="text-sm font-semibold">订阅地址</h3>{link ? <code className="bg-muted/50 my-2.5 block max-h-32 overflow-auto rounded-lg border p-3 text-xs whitespace-pre-wrap wrap-anywhere" tabIndex={0}>{link}</code> : <p className="text-muted-foreground mt-3 text-xs leading-relaxed mb-2.5">获取成功后显示当前订阅地址。</p>}<Button variant="outline" size="sm" disabled={busy || !link || current?.status === 'format_unavailable'} onClick={() => void perform('address')}><CopyIcon aria-hidden="true" />复制订阅地址</Button></section>
      <section className="mt-5 border-t pt-5" aria-label="配置内容"><h3 className="text-sm font-semibold">获取配置</h3><p className="text-muted-foreground mt-3 text-xs leading-relaxed">每次获取均重新检查当前授权、套餐和部署状态。预览、复制和下载只包含此用户的可用节点。</p><div className="mt-3.5 flex flex-wrap gap-2.5"><Button variant="outline" disabled={busy || !ready} onClick={() => preview ? setPreview(false) : void perform('preview')}>{preview && ready ? '收起预览' : '预览配置'}</Button><Button variant="outline" disabled={busy || !ready} onClick={() => void perform('copy')}><CopyIcon aria-hidden="true" />复制配置</Button><Button disabled={busy || !ready} onClick={() => void perform('download')}><ArrowDownIcon aria-hidden="true" />下载文件</Button></div>{preview && ready && <Textarea data-slot="subscription-preview" aria-label="配置预览" className="bg-muted/50 mt-4 block max-h-[420px] min-h-[200px] w-full resize-y font-mono text-xs leading-relaxed" readOnly spellCheck={false} value={current.content ?? ''} />}</section>
      <p className="text-muted-foreground text-xs leading-relaxed mt-5 flex items-start gap-2"><LockIcon className="size-3.5 mt-1" aria-hidden="true" />链接和配置包含连接凭据，需代理写入权限及五分钟内的管理员再次验证；只读诊断仅显示授权和状态。请仅提供给此用户。</p>
    </div>
    <footer><Button variant="destructive" disabled={busy} onClick={onReset}>重置订阅链接</Button><Button variant="outline" onClick={onClose}>完成</Button></footer>
  </Modal>
}
