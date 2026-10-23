import { useState } from 'react'
import { Field } from '../../components'
import type { NodeSettings } from './nodeSettingsForm'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'

export default function ExtraProtocolFields({ protocol, settings }: { protocol: string; settings?: NodeSettings }) {
  const [masquerade, setMasquerade] = useState(Boolean(settings?.hysteria2?.masquerade))
  const [statusCode, setStatusCode] = useState(String(settings?.hysteria2?.masquerade?.status_code ?? 200))
  const [contentType, setContentType] = useState(settings?.hysteria2?.masquerade?.content_type || 'text/plain')
  const [content, setContent] = useState(settings?.hysteria2?.masquerade?.content ?? '')
  const [multiplex, setMultiplex] = useState(settings?.shadowsocks?.multiplex.enabled ?? false)
  const [muxProtocol, setMuxProtocol] = useState(settings?.shadowsocks?.multiplex.protocol ?? 'h2mux')
  const [padding, setPadding] = useState(settings?.shadowsocks?.multiplex.padding ?? false)
  const [connections, setConnections] = useState(String(settings?.shadowsocks?.multiplex.max_connections ?? ''))
  const [minStreams, setMinStreams] = useState(String(settings?.shadowsocks?.multiplex.min_streams ?? ''))
  const [maxStreams, setMaxStreams] = useState(String(settings?.shadowsocks?.multiplex.max_streams ?? ''))
  const [limit, setLimit] = useState(settings?.shadowsocks?.multiplex.max_streams != null ? 'streams' : 'connections')
  if (protocol === 'hysteria2') return <>
    <Field label="BBR 档位" hint="仅在协商使用 BBR 时生效。"><NativeSelect className="w-full" name="bbr_profile" defaultValue={settings?.hysteria2?.bbr_profile ?? 'standard'}><option value="standard">标准</option><option value="conservative">保守</option><option value="aggressive">激进</option></NativeSelect></Field>
    <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="masquerade_enabled" checked={masquerade} onCheckedChange={checked => setMasquerade(checked === true)} className="mt-0.5" /><span>自定义伪装响应<small className="text-muted-foreground mt-1.5 block text-xs leading-relaxed">服务端向未认证请求返回指定内容。</small></span></label>
    {masquerade && <>
      <Field label="响应状态码"><Input name="masquerade_status_code" type="number" min={200} max={599} step={1} required value={statusCode} onChange={event => setStatusCode(event.target.value)} /></Field>
      <Field label="响应内容类型" hint={statusCode !== '200' ? '非 200 响应的内容类型由当前运行时决定。' : undefined}><Input disabled={statusCode !== '200'} name="masquerade_content_type" maxLength={128} required value={contentType} onChange={event => setContentType(event.target.value)} placeholder="text/plain" /></Field>
      <Field label="伪装响应内容" hint={['204', '304'].includes(statusCode) ? '此状态码不包含响应正文。' : undefined}><Textarea disabled={['204', '304'].includes(statusCode)} name="masquerade_content" rows={4} maxLength={16384} value={content} onChange={event => setContent(event.target.value)} /></Field>
    </>}
  </>
  if (protocol === 'tuic') return <Field label="客户端 UDP 转发"><NativeSelect className="w-full" name="udp_relay_mode" defaultValue={settings?.tuic?.udp_relay_mode ?? 'native'}><option value="native">原生 UDP（默认）</option><option value="quic-stream">QUIC 流</option><option value="udp-over-stream">可靠流（sing-box 扩展）</option></NativeSelect></Field>
  if (protocol === 'anytls') return <Field label="服务端填充策略" hint="每行一条规则，留空使用默认策略。"><Textarea name="padding_scheme" rows={5} maxLength={8192} defaultValue={settings?.anytls?.padding_scheme?.join('\n') ?? ''} placeholder={'stop=8\n0=30-30\n1=100-400'} spellCheck={false} /></Field>
  if (protocol === 'snell-v6') return <>
    <Field label="流量整形模式"><NativeSelect className="w-full" name="snell_mode" defaultValue={settings?.snell?.mode ?? 'default'}><option value="default">默认</option><option value="unshaped">关闭整形</option><option value="unsafe-raw">原始传输（不安全）</option></NativeSelect></Field>
    <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="snell_reuse" defaultChecked={settings?.snell?.reuse ?? false} className="mt-0.5" /><span>客户端连接复用</span></label>
  </>
  if (protocol === 'shadowsocks2022') return <>
    <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="udp_over_tcp" defaultChecked={settings?.shadowsocks?.udp_over_tcp ?? false} className="mt-0.5" /><span>客户端 UDP over TCP<small className="text-muted-foreground mt-1.5 block text-xs leading-relaxed">将 UDP 请求放入 TCP 连接。</small></span></label>
    <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="multiplex_enabled" checked={multiplex} onCheckedChange={checked => setMultiplex(checked === true)} className="mt-0.5" /><span>启用多路复用</span></label>
    {multiplex && <>
      <Field label="复用协议"><NativeSelect className="w-full" name="multiplex_protocol" value={muxProtocol} onChange={event => setMuxProtocol(event.target.value)}><option value="h2mux">h2mux</option><option value="smux">smux</option><option value="yamux">yamux</option></NativeSelect></Field>
      <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="multiplex_padding" checked={padding} onCheckedChange={checked => setPadding(checked === true)} className="mt-0.5" /><span>启用复用填充</span></label>
      <Field label="连接分配方式"><NativeSelect className="w-full" name="multiplex_limit" value={limit} onChange={event => setLimit(event.target.value)}><option value="connections">限制连接数量</option><option value="streams">限制每条连接流数</option></NativeSelect></Field>
      {limit === 'connections' ? <>
        <Field label="最大连接数"><Input name="max_connections" type="number" min={1} max={1024} step={1} value={connections} onChange={event => setConnections(event.target.value)} placeholder="默认" /></Field>
        <Field label="最少流数"><Input name="min_streams" type="number" min={1} max={1024} step={1} value={minStreams} onChange={event => setMinStreams(event.target.value)} placeholder="默认" /></Field>
      </> : <Field label="每条连接最大流数"><Input name="max_streams" type="number" min={1} max={1024} step={1} value={maxStreams} onChange={event => setMaxStreams(event.target.value)} placeholder="默认" /></Field>}
    </>}
  </>
  return null
}
