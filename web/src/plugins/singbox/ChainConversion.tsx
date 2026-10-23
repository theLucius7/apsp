import { useState } from 'react'
import { api } from '../../api'
import { Badge, ErrorNotice } from '../../components'
import { time } from '../../format'
import { useAction } from '../../hooks'
import type { ConversionCheck, MixedConversion } from './resourceTypes'
import { Button } from '@/components/ui/button'

const stateNames: Record<MixedConversion['state'], string> = { preparing: '正在准备有序候选', switched: '入口已切换', completed: '已转换', reverted: '已退回混合链路' }
const stateTones = { preparing: 'neutral', switched: 'neutral', completed: 'good', reverted: 'warm' } as const

function progress(conversion: MixedConversion) {
  if (conversion.state === 'preparing') return `混合路径第 ${conversion.mixed_generation} 代继续承载用户；有序第 ${conversion.ordered_generation} 代正在准备依赖和路径探测。`
  if (conversion.state === 'switched') return `入口已切换到有序第 ${conversion.ordered_generation} 代；混合第 ${conversion.mixed_generation} 代保留到恢复屏障完成。`
  if (conversion.state === 'completed') return `已转换为有序链路第 ${conversion.ordered_generation} 代。`
  return `第 ${conversion.attempts} 次转换未完成，混合路径保持不变：${conversion.last_error ?? '原因未记录'}`
}

// Mixed chains convert into ordered chains one chain at a time (ADR 0079 phase 3, S1d).
export default function ChainConversion({ base, conversion, writeError, onChanged }: { base: string; conversion?: MixedConversion | null; writeError: () => string; onChanged: () => void }) {
  const action = useAction()
  const [check, setCheck] = useState<ConversionCheck | null>(null)
  const running = conversion?.state === 'preparing' || conversion?.state === 'switched'
  const inspect = () => {
    if (action.busy || writeError()) return
    setCheck(null)
    void action.run(() => api<ConversionCheck>(`${base}/conversion`), setCheck)
  }
  const start = () => {
    if (action.busy || writeError() || !check?.ready || check.mixed_generation === null) return
    void action.run(() => api<ConversionCheck>(`${base}/conversion`, 'POST', { expected_generation: check.mixed_generation }), () => { setCheck(null); onChanged() })
  }
  return <section className="mt-6 grid gap-2" aria-label="转为有序链路">
    <h3 className="m-0 text-sm font-semibold">转为有序链路</h3>
    {conversion && <p className="bg-muted/60 text-foreground mb-4 justify-between rounded-lg border px-3 py-2.5 text-sm leading-relaxed flex flex-wrap items-center gap-2" role="status"><Badge tone={stateTones[conversion.state]}>{stateNames[conversion.state]}</Badge><span>{progress(conversion)}</span><small>开始于 {time(conversion.started_at)}</small></p>}
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">转换会在设备上真实切换一次。转换期间混合路径继续承载用户；有序候选通过路径探测后才切换入口，切换前失败会自动退回混合链路，切换后只能走有序链路自己的恢复流程。链路编号、入口、授权和计量不变，内部中继标识会更换。</p>
    {check && (check.ready
      ? <p className="bg-muted/60 text-foreground mb-4 justify-between rounded-lg border px-3 py-2.5 text-sm leading-relaxed flex flex-wrap items-center gap-2" role="status">可以转换：将以混合第 {check.mixed_generation} 代为基础创建有序候选。</p>
      : <ul className="list-disc pl-5 leading-relaxed" aria-label="暂不能转换的原因">{check.messages.map((message, index) => <li key={index}>{message}</li>)}</ul>)}
    <ErrorNotice message={action.error} />
    <div className="flex flex-wrap items-center justify-end gap-2">
      <Button type="button" variant="outline" size="sm" disabled={action.busy || running || Boolean(writeError())} onClick={inspect}>检查转换条件</Button>
      <Button type="button" size="sm" disabled={action.busy || running || !check?.ready || Boolean(writeError())} onClick={start}>开始转换</Button>
    </div>
  </section>
}
