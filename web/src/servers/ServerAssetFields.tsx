import { Field } from '../components'
import { trafficModes, trafficUnits } from '../server-assets'
import type { AssetDraft, TrafficUnit } from '../server-assets'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { SegmentedButton, SegmentedNav } from '@/components/segmented'

export function SetupNavigation({ monitoring = false }: { monitoring?: boolean }) {
  const sections = [['setup-basics', '名称'], ['setup-labels', '地区标签'], ['setup-cost', '成本到期'], ['setup-traffic', '流量额度'], ['setup-operations', '告警下载'], ...(monitoring ? [['setup-monitoring', '监控'], ['setup-probes', '拨测']] : [])]
  return <div className="bg-popover/95 supports-backdrop-filter:backdrop-blur-sm sticky top-0 z-[2] border-b px-4 py-2 sm:px-7">
    <SegmentedNav aria-label="配置分区">{sections.map(([id, title]) => <SegmentedButton type="button" key={id} onClick={() => document.getElementById(id)?.closest('section')?.scrollIntoView({ block: 'start' })}>{title}</SegmentedButton>)}</SegmentedNav>
  </div>
}

export default function ServerAssetFields({ value, onChange }: { value: AssetDraft; onChange: (value: AssetDraft) => void }) {
  const update = (change: Partial<AssetDraft>) => onChange({ ...value, ...change })
  return <>
    <section className="bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4" aria-labelledby="setup-labels">
      <div className="mb-4 flex items-center justify-between gap-3.5 max-sm:flex-wrap"><div className="min-w-0"><h3 className="flex items-center gap-2 text-sm font-semibold" id="setup-labels">地区与标签</h3><p className="text-muted-foreground mt-1 text-xs leading-relaxed">在展示页标记位置、归类和筛选服务器。</p></div><span className="bg-muted text-muted-foreground inline-block rounded border px-1.5 text-xs font-normal whitespace-nowrap">可选</span></div>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field className="mb-0" label="地区代码"><Input maxLength={16} value={value.region} onChange={event => update({ region: event.target.value.toUpperCase() })} placeholder="例如：CN / JP / DE" /></Field>
        <Field className="mb-0" label="展示分组"><Input maxLength={40} value={value.group_name} onChange={event => update({ group_name: event.target.value })} placeholder="例如：主力 / 备用" /></Field>
      </div>
      <div className="mt-4"><Field label="标签" hint="用逗号分隔，最多 16 个，每个最多 32 字。"><Input value={value.tags} maxLength={544} onChange={event => update({ tags: event.target.value })} placeholder="例如：主力, 线路:BGP" /></Field></div>
      <label className="relative flex cursor-pointer items-center gap-4 border-b py-3.5 last:border-0 last:pb-0"><span className="min-w-0 flex-1"><strong className="block text-sm font-medium">在展示页隐藏</strong><small className="text-muted-foreground mt-1 block text-xs leading-relaxed">后台管理仍可查看和编辑。</small></span><Switch checked={value.hidden} onCheckedChange={checked => update({ hidden: checked })} /></label>
    </section>
    <section className="bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4" aria-labelledby="setup-cost">
      <div className="mb-4 flex items-center justify-between gap-3.5 max-sm:flex-wrap"><div className="min-w-0"><h3 className="flex items-center gap-2 text-sm font-semibold" id="setup-cost">成本与到期</h3><p className="text-muted-foreground mt-1 text-xs leading-relaxed">记录服务器费用与下次到期日期。</p></div><span className="bg-muted text-muted-foreground inline-block rounded border px-1.5 text-xs font-normal whitespace-nowrap">可选</span></div>
      <div className="grid gap-4 max-sm:grid-cols-2 sm:grid-cols-3">
        <Field className="mb-0" label="每周期金额" hint="留空表示未填写，0 表示免费。"><Input type="number" min={0} max={1000000000} step="0.01" value={value.price} onChange={event => update({ price: event.target.value })} placeholder="未填写" /></Field>
        <Field className="mb-0" label="币种"><Input required pattern="[A-Za-z]{3}" maxLength={3} value={value.currency} onChange={event => update({ currency: event.target.value.toUpperCase() })} list="server-currencies" /><datalist id="server-currencies">{['CNY', 'USD', 'EUR', 'HKD', 'JPY', 'GBP', 'SGD'].map(code => <option key={code} value={code} />)}</datalist></Field>
        <Field className="mb-0" label="费用周期（天）" hint="0 为一次性；可输入自定义天数。"><Input required type="number" min={0} max={3650} value={value.billing_cycle} onChange={event => update({ billing_cycle: event.target.value, ...(Number(event.target.value) === 0 ? { auto_renewal: false } : {}) })} list="server-billing-cycles" /><datalist id="server-billing-cycles"><option value="30">30 天</option><option value="90">90 天</option><option value="180">180 天</option><option value="365">365 天</option><option value="0">一次性</option></datalist></Field>
      </div>
      <div className="mt-4"><Field label="到期日期（UTC）" hint="到期时间按所选日期的 UTC 零点记录。"><Input type="date" min="1970-01-01" max="9999-12-31" value={value.expiry} onChange={event => update({ expiry: event.target.value, ...(!event.target.value ? { auto_renewal: false } : {}) })} /></Field></div>
      <label className="relative flex cursor-pointer items-center gap-4 border-b py-3.5 last:border-0 last:pb-0"><span className="min-w-0 flex-1"><strong className="block text-sm font-medium">自动顺延到期记录</strong><small className="text-muted-foreground mt-1 block text-xs leading-relaxed">到期后按费用周期更新日期记录；不会向供应商付款。需先设置到期日期与非零周期。</small></span><Switch checked={value.auto_renewal} disabled={!value.expiry || Number(value.billing_cycle) <= 0} onCheckedChange={checked => update({ auto_renewal: checked })} /></label>
    </section>
    <section className="bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4" aria-labelledby="setup-traffic">
      <div className="mb-4 flex items-center justify-between gap-3.5 max-sm:flex-wrap"><div className="min-w-0"><h3 className="flex items-center gap-2 text-sm font-semibold" id="setup-traffic">流量额度</h3><p className="text-muted-foreground mt-1 text-xs leading-relaxed">按账单日汇总网卡观测流量，展示本期使用与剩余额度。</p></div><span className="bg-muted text-muted-foreground inline-block rounded border px-1.5 text-xs font-normal whitespace-nowrap">可选</span></div>
      <div className="grid gap-4 max-sm:grid-cols-2 sm:grid-cols-3">
        <div><Field className="mb-0" label="每期流量额度"><Input required inputMode="decimal" pattern="[0-9]+(\.[0-9]{1,6})?" value={value.amount} onChange={event => update({ amount: event.target.value })} /></Field><NativeSelect className="mt-1.5 w-full" aria-label="流量额度单位" value={value.unit} onChange={event => update({ unit: event.target.value as TrafficUnit })}>{Object.keys(trafficUnits).map(unit => <option key={unit} value={unit}>{unit}</option>)}</NativeSelect></div>
        <Field className="mb-0" label="流量统计口径"><NativeSelect className="w-full" value={value.traffic_limit_type} onChange={event => update({ traffic_limit_type: event.target.value as AssetDraft['traffic_limit_type'] })}>{Object.entries(trafficModes).map(([key, label]) => <option key={key} value={key}>{label}</option>)}</NativeSelect></Field>
        <Field className="mb-0" label="每月重置日（UTC）" hint="1–31 日；短月按当月最后一天。"><Input type="number" min={1} max={31} required value={value.reset_day} onChange={event => update({ reset_day: event.target.value })} /></Field>
      </div>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">0 表示不设额度。GB / TB 为十进制，GiB / TiB 为二进制。超额仅标记状态。</p>
      <div className="mt-4"><Field label="统计网卡" hint="留空统计所有上报网卡；支持 * 匹配和 ! 排除，逗号分隔。建议指定出口网卡，避免虚拟与回环接口重复统计。"><Input maxLength={1040} value={value.network_interface} onChange={event => update({ network_interface: event.target.value })} placeholder="例如：eth*,!eth1" /></Field></div>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">从首个有效采样建立基线；接入前、重启或缺失期间的流量可能不完整，观测值可能与供应商账单不同。修改口径或网卡选择会按已有记录重新汇总。</p>
    </section>
  </>
}
