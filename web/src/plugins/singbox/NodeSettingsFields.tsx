import { useState } from 'react'
import { Field } from '../../components'
import type { Node } from '../../types'

export { nodeSettingsRequest } from './nodeSettingsForm'
export type { NodeSettings } from './nodeSettingsForm'
import TransportFields from './NodeTransportFields'
import ExtraProtocolFields from './ExtraProtocolFields'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'

export function ConnectionFields({ node }: { node: Node | 'new' }) {
  const settings = node === 'new' ? undefined : node.settings
  return <>
    <Field label="监听地址" hint="填写服务器本机 IP。:: 监听 IPv6，通常同时接受 IPv4；仅 IPv4 可填 0.0.0.0。"><Input name="listen" required defaultValue={settings?.listen ?? '::'} maxLength={45} spellCheck={false} /></Field>
    <Field label="公开端口" hint="客户端连接使用的端口。留空跟随监听端口；有 NAT 映射时填写映射后的端口。"><Input name="public_port" type="number" min={1} max={65535} step={1} defaultValue={settings?.public_port ?? ''} placeholder="与监听端口相同" /></Field>
  </>
}

export function Seconds({ name, label, value, hint }: { name: string; label: string; value?: number | null; hint?: string }) {
  return <Field label={label} hint={hint ?? '留空使用运行时默认值。'}><Input name={name} type="number" min={1} max={3600} step={1} defaultValue={value ?? ''} placeholder="使用默认值" /></Field>
}

export default function ProtocolSettings({ protocol, node }: { protocol: string; node: Node | 'new' }) {
  const settings = node === 'new' ? undefined : node.settings
  const [obfs, setObfs] = useState(settings?.hysteria2?.obfs_enabled ?? false)
  const [bbr, setBbr] = useState(settings?.hysteria2?.ignore_client_bandwidth ?? false)
  const [disableKeepAlive, setDisableKeepAlive] = useState(settings?.disable_tcp_keep_alive ?? false)
  const tls = ['hysteria2', 'tuic', 'anytls', 'naive'].includes(protocol)
  const tcp = !['hysteria2', 'tuic'].includes(protocol)
  return <>
    <details className="my-4 rounded-lg border p-3.5 [&[open]>summary]:mb-5"><summary className="cursor-pointer text-sm font-semibold">监听与连接</summary><div className="grid gap-x-5 sm:grid-cols-2">
    {tcp && <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="tcp_fast_open" defaultChecked={settings?.tcp_fast_open ?? false} className="mt-0.5" /><span>TCP Fast Open<small className="text-muted-foreground mt-1.5 block text-xs leading-relaxed">默认关闭，需要服务器系统支持。</small></span></label>}
    {tcp && <>
      <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="disable_tcp_keep_alive" checked={disableKeepAlive} onCheckedChange={checked => setDisableKeepAlive(checked === true)} className="mt-0.5" /><span>关闭 TCP 保活</span></label>
      <fieldset className="contents m-0 min-w-0 border-0 p-0" disabled={disableKeepAlive}>
        <Seconds name="tcp_keep_alive_seconds" label="TCP 空闲保活（秒）" value={settings?.tcp_keep_alive_seconds} />
        <Seconds name="tcp_keep_alive_interval_seconds" label="TCP 保活间隔（秒）" value={settings?.tcp_keep_alive_interval_seconds} />
      </fieldset>
    </>}
    {!tcp && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">此协议使用 QUIC，无 TCP 监听参数。</p>}
    </div></details>
    {(tls || protocol === 'vless-reality') && <details className="my-4 rounded-lg border p-3.5 [&[open]>summary]:mb-5"><summary className="cursor-pointer text-sm font-semibold">TLS 设置</summary><div className="grid gap-x-5 sm:grid-cols-2">
    {tls && <>
      <Field label="最低 TLS 版本" hint={protocol === 'naive' ? '仅服务端使用。' : !tcp ? 'QUIC 使用 TLS 1.3。' : undefined}><NativeSelect className="w-full" name="tls_min_version" defaultValue={settings?.tls_min_version ?? ''}><option value="">默认</option><option value="1.2">TLS 1.2</option><option value="1.3">TLS 1.3</option></NativeSelect></Field>
      <Field label="最高 TLS 版本" hint={protocol === 'naive' ? '仅服务端使用。' : undefined}><NativeSelect className="w-full" name="tls_max_version" defaultValue={settings?.tls_max_version ?? ''}><option value="">默认</option>{tcp && <option value="1.2">TLS 1.2</option>}<option value="1.3">TLS 1.3</option></NativeSelect></Field>
    </>}
    {tcp && <Seconds name="tls_handshake_timeout_seconds" label="TLS 握手超时（秒）" value={settings?.tls_handshake_timeout_seconds} hint="服务端设置，留空使用默认值。" />}
    {tls && <Field label="TLS ALPN" hint={protocol === 'naive' ? '留空使用默认值；自定义仅支持 h2。' : '多个值用英文逗号分隔。留空沿用默认值，Hysteria2 / TUIC 默认为 h3。'}><Input name="tls_alpn" maxLength={263} defaultValue={settings?.tls_alpn?.join(', ') ?? ''} placeholder={['hysteria2', 'tuic'].includes(protocol) ? 'h3' : protocol === 'naive' ? 'h2' : '使用默认值'} autoComplete="off" /></Field>}
    </div></details>}
    {protocol === 'vless-reality' && <TransportFields settings={settings} />}
    <details className="my-4 rounded-lg border p-3.5 [&[open]>summary]:mb-5"><summary className="cursor-pointer text-sm font-semibold">协议高级设置</summary><div className="grid gap-x-5 sm:grid-cols-2">
    {protocol === 'vless-reality' && <>
      <Field label="Reality 握手目标" hint="留空跟随伪装域名。可指定域名或 IP，不含端口与路径。"><Input name="handshake_server" defaultValue={settings?.reality?.handshake_server ?? ''} placeholder="与伪装域名相同" maxLength={253} spellCheck={false} /></Field>
      <Field label="Reality 握手端口"><Input name="handshake_port" type="number" min={1} max={65535} defaultValue={settings?.reality?.handshake_port ?? 443} required /></Field>
      <Seconds name="max_time_difference_seconds" label="Reality 时间容差（秒）" value={settings?.reality?.max_time_difference_seconds} hint="服务端允许的客户端时钟偏差，留空不额外限制。" />
      <Field label="客户端 TLS 指纹" hint="同步写入 sing-box 配置与 VLESS 分享链接。"><NativeSelect className="w-full" name="fingerprint" defaultValue={settings?.reality?.fingerprint ?? 'chrome'}>{[['chrome', 'Chrome'], ['firefox', 'Firefox'], ['safari', 'Safari'], ['edge', 'Edge'], ['ios', 'iOS'], ['android', 'Android'], ['randomized', '随机']].map(([value, label]) => <option key={value} value={value}>{label}</option>)}</NativeSelect></Field>
    </>}
    {protocol === 'hysteria2' && <>
      <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="ignore_client_bandwidth" checked={bbr} onCheckedChange={checked => setBbr(checked === true)} className="mt-0.5" /><span>强制使用 BBR<small className="text-muted-foreground mt-1.5 block text-xs leading-relaxed">与手动带宽互斥。关闭且留空带宽时沿用运行时协商。</small></span></label>
      <Field label="服务器上传带宽（Mbps）" hint="对应客户端下行；上下行同时填写或同时留空。"><Input name="up_mbps" type="number" min={1} max={1000000} step={1} disabled={bbr} defaultValue={settings?.hysteria2?.up_mbps ?? ''} placeholder="不限制" /></Field>
      <Field label="服务器下载带宽（Mbps）" hint="对应客户端上行，订阅自动换算方向。"><Input name="down_mbps" type="number" min={1} max={1000000} step={1} disabled={bbr} defaultValue={settings?.hysteria2?.down_mbps ?? ''} placeholder="不限制" /></Field>
      <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="obfs_enabled" checked={obfs} onCheckedChange={checked => setObfs(checked === true)} className="mt-0.5" /><span>启用 Salamander 混淆<small className="text-muted-foreground mt-1.5 block text-xs leading-relaxed">服务端与授权客户端自动使用同一混淆参数。</small></span></label>
      {obfs && <Field label="混淆密码" hint={settings?.hysteria2?.obfs_enabled ? '已配置，留空保留；填写新值后客户端需更新订阅。' : '留空自动生成。也可填写 8 至 256 字节的密码。'}><Input name="obfs_password" type="password" minLength={8} maxLength={256} autoComplete="new-password" /></Field>}
    </>}
    {protocol === 'tuic' && <>
      <Field label="拥塞控制"><NativeSelect className="w-full" name="congestion_control" defaultValue={settings?.tuic?.congestion_control ?? 'cubic'}><option value="cubic">CUBIC（默认）</option><option value="bbr">BBR</option><option value="new_reno">New Reno</option></NativeSelect></Field>
      <Seconds name="auth_timeout_seconds" label="认证超时（秒）" value={settings?.tuic?.auth_timeout_seconds} />
      <Seconds name="heartbeat_seconds" label="心跳间隔（秒）" value={settings?.tuic?.heartbeat_seconds} hint="同时作用于服务器与客户端。" />
      <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="zero_rtt_handshake" defaultChecked={settings?.tuic?.zero_rtt_handshake ?? false} className="mt-0.5" /><span>启用 0-RTT 握手<small className="text-muted-foreground mt-1.5 block text-xs leading-relaxed">减少握手等待，但存在重放风险；默认关闭。</small></span></label>
    </>}
    {protocol === 'anytls' && <>
      <Seconds name="idle_session_check_seconds" label="闲置会话检查间隔（秒）" value={settings?.anytls?.idle_session_check_seconds} hint="客户端设置，留空使用 30 秒默认值。" />
      <Seconds name="idle_session_timeout_seconds" label="闲置会话超时（秒）" value={settings?.anytls?.idle_session_timeout_seconds} hint="客户端设置，留空使用 30 秒默认值。" />
      <Field label="最少保留闲置会话" hint="客户端设置，留空或 0 不额外保留。"><Input name="min_idle_session" type="number" min={0} max={128} step={1} defaultValue={settings?.anytls?.min_idle_session ?? ''} placeholder="0" /></Field>
    </>}
    <ExtraProtocolFields protocol={protocol} settings={settings} />
    {protocol === 'naive' && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">Naive 使用 HTTP/2，安全参数在 TLS 设置中配置。</p>}
  </div></details></>
}
