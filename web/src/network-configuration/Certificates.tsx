import { useState } from 'react'
import { api } from '../api'
import { Field } from '../components'
import { useResource } from '../hooks'
import type { Document, Target, Version } from './types'
import { time } from './types'
import AcmePlan from './AcmePlan'
import CertificateDeployments from './CertificateDeployments'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'
import { Panel } from '@/components/panel'

export default function Certificates({ document, run, changed }: { document: Document; run: (work: () => Promise<unknown>) => Promise<void>; changed: () => void }) {
  const versions = useResource<Version[]>(`/api/network-configuration/certificates/${document.id}/versions`)
  const challenges = useResource<{ id: string; name: string; status: string; error_code: string | null; expires_at: number }[]>(`/api/network-configuration/certificates/${document.id}/dns-challenges`)
  const [pem, setPem] = useState(''), [selectedDomain, setSelectedDomain] = useState(''), [value, setValue] = useState(''), [credential, setCredential] = useState('')
  const [confirmed, setConfirmed] = useState(false)
  const targets = document.config.targets as Target[]
  const renewal = document.config.renewal
  const isDns01 = typeof renewal === 'object' && renewal !== null && 'mode' in renewal && renewal.mode === 'dns01'
  return <><AcmePlan certificateId={document.id} run={run} /><CertificateDeployments document={document} run={run} /><Panel className="grid gap-3 p-5"><h3 className="text-sm font-semibold">签发、保存、部署与握手</h3><p className="text-muted-foreground mt-3 text-xs leading-relaxed">这里保存公开证书并选择期望版本；受管部署在独立预览中排队，外部部署由维护方执行。握手验证会从面板发起真实 TLS 连接，检查域名、信任链和指纹，适用于不同 TLS 服务。列表与日志均不保存私钥。</p>
    <Field label="公开 PEM 证书链"><Textarea className="w-full" rows={5} value={pem} onChange={event => setPem(event.target.value)} placeholder="仅粘贴公开证书；不接受私钥" /></Field>
    <Button variant="outline" disabled={!pem.trim()} onClick={() => void run(async () => { await api(`/api/network-configuration/certificates/${document.id}/versions`, 'POST', { public_chain: pem, revision: document.revision }); setPem(''); versions.reload(); changed() })}>检查并保存证书版本</Button>
    {versions.error && <p className="text-destructive text-sm" role="alert">{versions.error}</p>}
    <label className="flex items-center gap-2 text-sm"><Checkbox checked={confirmed} onCheckedChange={checked => setConfirmed(checked === true)} />确认改变期望版本，并由维护方完成实际部署</label>
    <div>{versions.data?.map(version => <article className="my-3 flex flex-col gap-2" key={version.id}><strong>{document.active_version === version.id ? '当前期望版本' : '历史版本'}</strong><span className="text-muted-foreground text-xs">{time(version.not_before)} → {time(version.not_after)}</span><code>{version.fingerprint}</code><Button variant="ghost" size="sm" disabled={!confirmed || document.active_version === version.id || version.not_after * 1000 < Date.now()} onClick={() => void run(async () => { await api(`/api/network-configuration/certificates/${document.id}/select-version`, 'POST', { version_id: version.id, revision: document.revision, confirmed }); changed() })}>选择此版本</Button></article>)}</div>
    {targets.map((target, index) => <div className="flex flex-wrap items-center justify-between gap-2.5 border-b py-2.5" key={`${target.server_id}:${target.domain}:${target.port}`}><span>{target.service} · {target.domain}:{target.port}</span><Button variant="outline" disabled={!document.active_version} onClick={() => void run(async () => { await api(`/api/network-configuration/certificates/${document.id}/verify`, 'POST', { target_index: index }); changed() })}>从面板核对实际证书</Button></div>)}
    {isDns01 && <><h3 className="text-sm font-semibold">DNS-01 验证辅助</h3><p className="text-muted-foreground mt-3 text-xs leading-relaxed">维护方先向签发服务下单，提供该订单的 DNS-01 摘要。apsp仅创建和清理受管 TXT；验证记录已发布不代表证书已签发。仅支持与凭据规则相同的域名，原有 TXT 保留。</p>
      <Field label="覆盖域名标识"><NativeSelect className="w-full" value={selectedDomain} onChange={event => setSelectedDomain(event.target.value)}><option value="">请选择</option>{(document.config.domain_ids as string[]).map(id => <option value={id} key={id}>{id}</option>)}</NativeSelect></Field>
      <Field label="签发方提供的 DNS-01 摘要"><Input value={value} maxLength={43} onChange={event => setValue(event.target.value)} /></Field>
      <Field label="凭据中心标识（可留空使用已有 DDNS 规则凭据）"><Input value={credential} onChange={event => setCredential(event.target.value)} /></Field>
      <Button variant="outline" disabled={!selectedDomain || value.length !== 43} onClick={() => void run(async () => { await api(`/api/network-configuration/certificates/${document.id}/dns-challenges`, 'POST', { domain_id: selectedDomain, value, credential_id: credential || null, expires_at: Math.floor(Date.now() / 1000) + 3600 }); setValue(''); challenges.reload() })}>登记一小时验证挑战</Button>
      {challenges.error && <p className="text-destructive text-sm" role="alert">{challenges.error}</p>}
      {challenges.data?.map(challenge => <div className="flex flex-wrap items-center justify-between gap-2.5 border-b py-2.5" key={challenge.id}><span>{challenge.name} · {({ pending: '等待发布', presented: '提供方已接收', unknown: '结果未知，需核对', cleaned: '已清理' } as Record<string, string>)[challenge.status] ?? '状态未知'} · {time(challenge.expires_at)}</span><div><Button variant="ghost" size="sm" disabled={challenge.status === 'cleaned' || challenge.expires_at * 1000 <= Date.now()} onClick={() => void run(async () => { await api(`/api/network-configuration/dns-challenges/${challenge.id}/present`, 'POST'); challenges.reload() })}>发布或核对 TXT</Button><Button variant="ghost" size="sm" disabled={challenge.status === 'cleaned'} onClick={() => void run(async () => { await api(`/api/network-configuration/dns-challenges/${challenge.id}/cleanup`, 'POST'); challenges.reload() })}>仅清理本挑战 TXT</Button></div></div>)}
    </>}
  </Panel></>
}
