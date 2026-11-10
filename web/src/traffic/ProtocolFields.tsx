import { useState } from 'react'
import { Field } from '../components'
import type { Node } from '../types'
import ProtocolSettings from './NodeSettingsFields'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'

export const protocolNames: Record<string, string> = {
  shadowtls: 'Shadowsocks + ShadowTLS v3', 'vless-tls': 'VLESS + TLS', 'vless-tunnel': 'VLESS + 隧道', trojan: 'Trojan', vmess: 'VMess', http: 'HTTPS 代理', mixed: 'HTTP / SOCKS 混合（TLS）', socks: 'SOCKS5', shadowsocks: 'Shadowsocks AEAD',
  'vless-reality': 'VLESS + Reality', hysteria2: 'Hysteria2', shadowsocks2022: 'Shadowsocks 2022',
  tuic: 'TUIC v5', anytls: 'AnyTLS', naive: 'Naive（HTTP/2）', 'snell-v6': 'Snell v6',
}

// Protocols that appear on existing nodes but are not offered for creation:
// tunnel origins arrive through the Nexus takeover (ADR 0109).
const displayOnly: Record<string, string> = { 'vless-tunnel': 'VLESS + 隧道（WS）' }

export function protocolName(protocol: string) {
  return protocolNames[protocol] ?? displayOnly[protocol] ?? protocol
}

export function protocolRequest(form: FormData) {
  const type = String(form.get('protocol'))
  if (['shadowsocks2022', 'shadowsocks', 'shadowtls'].includes(type)) return { type, method: String(form.get('method')) }
  if (!['hysteria2', 'tuic', 'anytls', 'naive', 'vless-tls', 'trojan', 'vmess', 'http', 'mixed'].includes(type)) return { type }
  const mode = String(form.get('tls_mode'))
  if (mode === 'reuse') return { type, tls: { mode, node_id: Number(form.get('certificate_node_id')) } }
  if (mode === 'dns01') return { type, tls: { mode, email: String(form.get('email')).trim(), domains: String(form.get('domains')).split(/[\s,]+/).filter(Boolean), ...(form.get('api_token') ? { api_token: String(form.get('api_token')).trim() } : {}) } }
  if (mode === 'acme') return { type, tls: { mode, email: String(form.get('email')).trim(), challenge: String(form.get('challenge')) } }
  const certificate = String(form.get('certificate') ?? '').trim()
  const key = String(form.get('key') ?? '').trim()
  return { type, tls: { mode, ...(certificate || key ? { certificate, key } : {}) } }
}

export default function ProtocolFields({ node }: { node: Node | 'new' }) {
  const existing = node === 'new' ? null : node
  const [protocol, setProtocol] = useState(existing?.protocol ?? 'vless-reality')
  const tls = existing?.protocol_config?.tls
  const [mode, setMode] = useState<string>(tls?.mode ?? 'acme')
  const [challenge, setChallenge] = useState(tls?.challenge ?? 'http-01')
  const [email, setEmail] = useState(tls?.email ?? '')
  const [certificate, setCertificate] = useState('')
  const [privateKey, setPrivateKey] = useState('')
  const needsTls = ['hysteria2', 'tuic', 'anytls', 'naive', 'vless-tls', 'trojan', 'vmess', 'http', 'mixed'].includes(protocol)
  const reality = protocol === 'vless-reality'
  return <>
    <Field label="代理协议" hint={existing ? '协议创建后不可更改。需要更换协议时请创建新节点。' : '所有协议均支持完整 sing-box 配置订阅。'}>
      <NativeSelect className="w-full" aria-label="代理协议" value={protocol} disabled={Boolean(existing)} onChange={event => setProtocol(event.target.value)}>{Object.entries(protocolNames).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</NativeSelect>
      <input type="hidden" name="protocol" value={protocol} />
    </Field>
    {['shadowsocks','shadowtls'].includes(protocol) && <Field label="加密方法"><NativeSelect name="method" defaultValue={existing?.protocol_config?.method ?? 'aes-128-gcm'}>{['aes-128-gcm','aes-256-gcm','chacha20-ietf-poly1305','xchacha20-ietf-poly1305'].map(method => <option key={method}>{method}</option>)}</NativeSelect></Field>}
    {protocol === 'vless-tunnel' && <p className="text-sm text-muted-foreground">源站必须监听回环地址，使用 WebSocket 或 HTTPUpgrade；公开域名由隧道或 CDN 提供 TLS。</p>}
    {protocol === 'shadowsocks2022'  && <Field label="加密方法" hint="节点密钥和每个授权的密钥自动生成。加密方法创建后不可更改。">
      <NativeSelect className="w-full" name="method" defaultValue={existing?.protocol_config?.method ?? '2022-blake3-aes-128-gcm'} disabled={Boolean(existing)}><option value="2022-blake3-aes-128-gcm">AES-128-GCM</option><option value="2022-blake3-aes-256-gcm">AES-256-GCM</option></NativeSelect>
      {existing && <input type="hidden" name="method" value={existing.protocol_config?.method ?? '2022-blake3-aes-128-gcm'} />}
    </Field>}
    {(needsTls || reality || protocol === 'vless-tunnel' || protocol === 'shadowtls') && <Field label={reality ? '伪装域名（SNI）' : '证书域名（SNI）'} hint={reality ? '填写可从服务器访问、支持 TLS 的目标域名。' : '填写证书覆盖的完整域名；自动申请时此域名需解析到当前服务器。'}>
      <Input name="sni" required defaultValue={existing?.sni ?? ''} placeholder={reality ? 'www.example.com' : 'node.example.com'} autoComplete="off" spellCheck={false} />
    </Field>}
    {needsTls && <>
      <Field label="TLS 证书"><NativeSelect className="w-full" name="tls_mode" value={mode} onChange={event => setMode(event.target.value as 'acme' | 'manual' | 'dns01')}><option value="acme">自动申请和续期（Let's Encrypt）</option><option value="dns01">DNS-01（Cloudflare，支持通配符）</option><option value="manual">手动导入证书</option><option value="reuse">复用本机节点的证书</option></NativeSelect></Field>
      {mode === 'reuse' ? <Field label="证书来源节点编号" hint="复用同一服务器的证书配置。相同 DNS-01 配置共用签发与续期；SNI 必须在证书覆盖范围内。"><Input name="certificate_node_id" type="number" min={1} step={1} required /></Field> : mode === 'dns01' ? <>
        <Field label="联系邮箱"><Input name="email" type="email" required value={email} onChange={event => setEmail(event.target.value)} /></Field>
        <Field label="证书覆盖域名" hint="逗号或空格分隔，可填写 *.example.com；必须覆盖上方 SNI。"><Input name="domains" required defaultValue={tls?.domains?.join(', ') ?? existing?.sni ?? ''} /></Field>
        <Field label="Cloudflare DNS API Token" hint="需要目标区域的 DNS 编辑权限。保存后不回显；留空保留原令牌。"><Input name="api_token" type="password" autoComplete="new-password" required={tls?.mode !== 'dns01'} /></Field>
        <p className="text-sm text-muted-foreground">令牌将随证书配置发送到此受管服务器，供运行时签发和续期使用。不需要开放 TCP 80 或 443。</p>
      </> : mode === 'acme' ? <>
        <Field label="联系邮箱"><Input type="email" name="email" required maxLength={254} value={email} onChange={event => setEmail(event.target.value)} placeholder="admin@example.com" autoComplete="email" /></Field>
        <Field label="域名验证方式"><NativeSelect className="w-full" name="challenge" value={challenge} onChange={event => setChallenge(event.target.value as 'http-01' | 'tls-alpn-01')}><option value="http-01">HTTP-01（TCP 80）</option><option value="tls-alpn-01">TLS-ALPN-01（TCP 443）</option></NativeSelect></Field>
        <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{challenge === 'http-01' ? 'TCP 80' : 'TCP 443'} 需允许公网访问且未被其他服务占用。保存即授权使用 Let's Encrypt 自动签发和续期；同一服务器共用邮箱与验证方式。编辑现有自动证书时，这两项会同步修改该服务器的其他自动证书节点。首次签发最多等待 4 分钟，失败时部署回滚。</p>
      </> : <>
        <Field label="PEM 证书链" hint={tls?.mode === 'manual' ? '已配置证书。保持两项为空可保留；替换时需同时填写证书链和私钥。' : '包含服务器证书及必要的中间证书。'}><Textarea name="certificate" value={certificate} onChange={event => setCertificate(event.target.value)} rows={5} maxLength={65536} required={tls?.mode !== 'manual'} autoComplete="off" spellCheck={false} placeholder="-----BEGIN CERTIFICATE-----" /></Field>
        <Field label="PEM 私钥" hint="私钥仅用于服务器配置，保存后不会回显，也不会出现在客户端订阅中。"><Textarea name="key" value={privateKey} onChange={event => setPrivateKey(event.target.value)} rows={5} maxLength={16384} required={tls?.mode !== 'manual'} autoComplete="off" spellCheck={false} placeholder="-----BEGIN PRIVATE KEY-----" /></Field>
      </>}
    </>}
    <ProtocolSettings key={protocol} protocol={protocol} node={node} />
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{reality ? 'Reality 密钥和 short ID 自动生成。' : protocol === 'snell-v6' ? '使用 Snell v6 多用户模式，节点与授权密钥自动生成。客户端需支持 Snell v6。' : '授权凭据自动生成。新增协议请使用 sing-box 配置订阅；Naive 客户端需带 Naive 支持。'}{['hysteria2', 'tuic'].includes(protocol) ? ' 此协议使用 UDP，请确保节点的 UDP 端口可达。' : protocol === 'shadowsocks2022' ? ' 节点使用 TCP 和 UDP。' : ' 节点使用 TCP。'}</p>
  </>
}
