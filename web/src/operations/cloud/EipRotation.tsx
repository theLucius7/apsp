import { useState } from 'react'
import { api } from '../../api'
import { ErrorNotice, Field, Modal } from '../../components'
import { useAction } from '../../hooks'
import { charge } from '../../resources/cloud/types'
import type { EipRotation, Resource } from '../../resources/cloud/types'
import { checked, resourceError, rotationError } from './guards'
import type { CurrentCloud } from './guards'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { isPort } from '@/lib/validate'

const validAttempts = (value: number) => Number.isInteger(value) && value >= 1 && value <= 12

/** Reads the instance's EIP into a preview, then starts the rotation only on an explicit confirmation. */
export function EipRotationEditor({ resource, close, saved, current }: { resource: Resource; close: () => void; saved: () => void; current: CurrentCloud }) {
  const [port, setPort] = useState(443), [attempts, setAttempts] = useState(5)
  const [preview, setPreview] = useState<EipRotation | null>(null), [confirmed, setConfirmed] = useState(false), action = useAction()
  const writeError = () => resourceError(current, resource, { managed: true, idle: true })
  const submit = () => {
    if (preview) {
      if (confirmed) void action.run(() => checked(writeError, () => api(`/api/resources/alicloud/eip-rotations/${preview.id}/confirm`, 'POST', { confirm: true, snapshot_digest: preview.snapshot_digest })), saved)
      return
    }
    void action.run(() => checked(writeError, () => api<EipRotation>(`/api/resources/alicloud/resources/${resource.id}/eip-rotation/preview`, 'POST', { probe_port: port, max_attempts: attempts, resource_revision: resource.revision })), value => { setPreview(value); setConfirmed(false) })
  }
  return <Modal title={`更换公网 IP · ${resource.name}`} onClose={close} busy={action.busy}><form onSubmit={event => { event.preventDefault(); submit() }}><div data-slot="modal-body" className="p-4"><ErrorNotice message={action.error || writeError()} />{preview ? <>
    <dl className="my-5 grid gap-4 sm:grid-cols-2"><div><dt className="text-muted-foreground mb-1.5 text-xs">当前地址</dt><dd className="m-0 wrap-anywhere">{preview.before.public_ip}<small className="text-muted-foreground mt-1.5 block wrap-anywhere">{preview.before.allocation_id}</small></dd></div><div><dt className="text-muted-foreground mb-1.5 text-xs">新地址沿用</dt><dd className="m-0 wrap-anywhere">{preview.before.isp} · {charge(preview.before.internet_charge_type)}<small className="text-muted-foreground mt-1.5 block wrap-anywhere">{preview.before.package_id ? `共享带宽 ${preview.before.package_id}` : `${preview.before.bandwidth} Mbps`}</small></dd></div><div><dt className="text-muted-foreground mb-1.5 text-xs">国内探测</dt><dd className="m-0 wrap-anywhere">TCP {preview.probe_port} 端口</dd></div><div><dt className="text-muted-foreground mb-1.5 text-xs">最多尝试</dt><dd className="m-0 wrap-anywhere">{preview.max_attempts} 个新地址</dd></div></dl>
    <ul className="text-muted-foreground my-3 grid list-disc gap-1.5 pl-5 text-sm leading-relaxed"><li>每次切换时实例的公网 IPv4 会断开几秒，已建立的连接会中断；IPv6 不受影响。</li><li>新地址可用后，<strong className="text-foreground font-medium">旧地址会被释放且无法找回</strong>。所有新地址都不可用时，实例绑回旧地址。</li><li>写着旧 IP 的节点公开地址和客户端配置不会自动修改，完成后需改为新地址；配置了域名解析的服务器会在 Agent 上报新地址后自动更新。</li><li>新地址的 IP 和端口会发送给公共探测服务 Globalping，用于从国内测试连通性。</li><li>绑定在实例上的 EIP 通常不收配置费；未绑定的地址闲置期间可能按小时收费，具体金额以阿里云账单为准。</li></ul>
    <label className="my-4 flex items-start gap-2.5 text-sm leading-relaxed"><Checkbox checked={confirmed} onCheckedChange={checked => setConfirmed(checked === true)} className="mt-0.5" />确认更换，并释放 {preview.before.public_ip}</label>
  </> : <fieldset disabled={action.busy}>
    <Field label="探测端口" hint="实例上正在监听、安全组已放行的 TCP 端口，用来判断新地址从国内能否连接。"><Input required type="number" min={1} max={65535} step={1} value={port} onChange={event => setPort(Number(event.target.value))} /></Field>
    <Field label="最多尝试几个新地址" hint="超过后放弃，实例绑回原地址。"><Input required type="number" min={1} max={12} step={1} value={attempts} onChange={event => setAttempts(Number(event.target.value))} /></Field>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">下一步只读取云端配置并生成预览，确认后才会申请新地址。</p>
  </fieldset>}</div><footer><Button variant="outline" type="button" disabled={action.busy} onClick={preview ? () => { setPreview(null); setConfirmed(false) } : close}>{preview ? '重新预览' : '取消'}</Button><Button type="submit" disabled={action.busy || Boolean(writeError()) || (preview ? !confirmed : !isPort(port) || !validAttempts(attempts))}>{action.busy ? '正在处理…' : preview ? '确认更换' : '读取并预览'}</Button></footer></form></Modal>
}

/** Closes a halted rotation on the administrator's word; nothing is bound or released by it. */
export function EipRotationReconcile({ rotation, current, close, saved }: { rotation: EipRotation; current: CurrentCloud; close: () => void; saved: () => void }) {
  const [verified, setVerified] = useState(false), [evidence, setEvidence] = useState(''), action = useAction()
  const size = new TextEncoder().encode(evidence.trim()).length
  const error = () => rotationError(current, rotation, ['unknown'])
  return <Modal title="结束人工核对" onClose={close} busy={action.busy}><div data-slot="modal-body" className="p-4"><ErrorNotice message={action.error || error()} /><p>自动操作已停止，结束跟踪不会绑定或释放任何地址。请先到<a className="text-primary underline-offset-4 hover:underline" href="https://vpc.console.aliyun.com/" target="_blank" rel="noreferrer">阿里云 EIP 控制台</a>核对实例绑定的地址，并释放不再使用的地址，否则闲置会产生费用。</p>
    {rotation.attempts.length > 0 && <p className="text-muted-foreground my-3 text-sm wrap-anywhere">本次申请过的地址：{rotation.attempts.map(item => `${item.ip}（${item.allocation_id}）`).join('、')}；原地址：{rotation.before.public_ip}（{rotation.before.allocation_id}）。</p>}
    <label className="my-4 flex items-start gap-2.5 text-sm leading-relaxed"><Checkbox checked={verified} onCheckedChange={checked => setVerified(checked === true)} className="mt-0.5" />已在云端核对并处理完毕</label>
    <Field label="核对方法与结论（32–4096 字节，不要填写凭据）"><Textarea className="min-h-20 w-full" value={evidence} maxLength={4096} onChange={event => setEvidence(event.target.value)} /></Field>
  </div><footer><Button variant="outline" disabled={action.busy} onClick={close}>返回</Button><Button disabled={action.busy || !verified || size < 32 || size > 4096 || Boolean(error())} onClick={() => void action.run(() => checked(error, () => api(`/api/resources/alicloud/eip-rotations/${rotation.id}/reconcile`, 'POST', { process_stopped: true, evidence })), saved)}>结束跟踪</Button></footer></Modal>
}
