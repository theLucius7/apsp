import { useState } from 'react'
import { Field, FormDialog } from '../../components'
import { time } from '../../format'
import { addCalendar, fromLocalInput, localInput } from './validity'
import type { Until } from './validity'
import { SegmentedButton, SegmentedNav } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'

type Mode = 'extend' | 'at' | 'never'
type Unit = 'day' | 'month'
const presets: { label: string; amount: number; unit: Unit }[] = [
  { label: '1 个月', amount: 1, unit: 'month' }, { label: '3 个月', amount: 3, unit: 'month' },
  { label: '6 个月', amount: 6, unit: 'month' }, { label: '1 年', amount: 12, unit: 'month' },
]

/** Chooses a new expiry for the current package; the caller previews it before it applies. */
export default function ValidityDialog({ expiresAt, startsAt, bridged, onClose, onSubmit }: { expiresAt: number | null; startsAt: number | null; bridged: boolean; onClose: () => void; onSubmit: (until: Until) => void }) {
  const now = Math.floor(Date.now() / 1000)
  const expired = expiresAt !== null && expiresAt <= now
  const [mode, setMode] = useState<Mode>(expiresAt === null ? 'at' : 'extend')
  const [amount, setAmount] = useState('1')
  const [unit, setUnit] = useState<Unit>('month')
  const [moment, setMoment] = useState(() => localInput(expiresAt !== null && !expired ? expiresAt : addCalendar(now, 1, 'month')))
  // Extending counts from the current expiry, or from now once it has passed.
  const base = expiresAt !== null && !expired ? expiresAt : now
  const count = Number(amount)
  const extended = Number.isInteger(count) && count >= 1 && count <= (unit === 'month' ? 1200 : 36500) ? addCalendar(base, count, unit) : null
  const chosen = mode === 'extend' ? extended : mode === 'at' ? fromLocalInput(moment) : null
  const problem = mode === 'never'
    ? (expiresAt === null ? '当前已经不限期' : '')
    : chosen === null ? (mode === 'extend' ? '请输入 1 至 1200 个月或 1 至 36500 天' : '请选择完整的日期和时间')
      : chosen <= now ? '到期时间必须晚于当前时间'
        : startsAt !== null && chosen <= startsAt ? '到期时间必须晚于套餐开始时间'
          // The picker works to the minute, so an unchanged field must not count as a change.
          : expiresAt !== null && Math.floor(chosen / 60) === Math.floor(expiresAt / 60) ? '到期时间没有变化' : ''
  const submit = () => {
    if (problem) return
    onSubmit(mode === 'never' ? 'never' : { at: chosen! })
  }
  return <FormDialog title="调整有效期" onClose={onClose} onSubmit={submit} busy={false} submitDisabled={Boolean(problem)} submitLabel="预览">
    <p className="mb-3 text-[13px]">当前：{expiresAt === null ? '不限期' : `${time(expiresAt)}${expired ? '（已到期）' : ''}`}</p>
    <SegmentedNav aria-label="调整方式" className="mb-4">
      {expiresAt !== null && <SegmentedButton aria-pressed={mode === 'extend'} onClick={() => setMode('extend')}>延长</SegmentedButton>}
      <SegmentedButton aria-pressed={mode === 'at'} onClick={() => setMode('at')}>指定到期时间</SegmentedButton>
      <SegmentedButton aria-pressed={mode === 'never'} onClick={() => setMode('never')}>不限期</SegmentedButton>
    </SegmentedNav>
    {mode === 'extend' && <>
      <div className="mb-3 flex flex-wrap gap-1.5">{presets.map(preset => <Button key={preset.label} type="button" size="sm" variant={count === preset.amount && unit === preset.unit ? 'default' : 'outline'} onClick={() => { setAmount(String(preset.amount)); setUnit(preset.unit) }}>{preset.label}</Button>)}</div>
      <div className="grid grid-cols-[minmax(0,1fr)_7rem] gap-2">
        <Field label="延长"><Input name="amount" type="number" inputMode="numeric" min={1} max={unit === 'month' ? 1200 : 36500} value={amount} onChange={event => setAmount(event.target.value)} /></Field>
        <Field label="单位"><NativeSelect className="w-full" name="unit" value={unit} onChange={event => setUnit(event.target.value as Unit)}><option value="month">个月</option><option value="day">天</option></NativeSelect></Field>
      </div>
      <p className="text-muted-foreground text-xs leading-relaxed">{expired ? '已到期，从现在起算' : '从当前到期时间起算'}，新的到期时间：<strong className="text-foreground font-medium">{extended === null ? '—' : time(extended)}</strong></p>
    </>}
    {mode === 'at' && <Field label="到期时间" hint="按本机时间填写，可以提前或推后；到期后停止提供可用订阅。"><Input name="expires_at" type="datetime-local" step={60} required value={moment} onChange={event => setMoment(event.target.value)} /></Field>}
    {mode === 'never' && <p className="text-[13px] leading-relaxed">不再按时间到期；流量额度和重置周期不变。</p>}
    {problem && <p role="status" className="text-destructive mt-3 text-xs">{problem}</p>}
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">只改到期时间，不重置流量；下一步预览，确认后生效。{bridged && '迁移用户的新到期时间会同步到原系统，通常十几秒内生效。'}</p>
  </FormDialog>
}
