import { useId, useRef, useState } from 'react'
import { ChevronDownIcon, PlusIcon } from 'lucide-react'
import { api } from '../../api'
import { ErrorNotice, Field, FormDialog } from '../../components'
import { useAction } from '../../hooks'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { currencies, cycles, decimal, root } from './types'
import type { Expense, Label } from './types'

const kindNames: Record<Label['kind'], string> = { category: '分类', payment_method: '付款方式', payer: '付款人' }

/** A label choice that can add a new label on the spot, without leaving the form. */
function LabelSelect({ kind, name, labels, selected, onCreated }: { kind: Label['kind']; name: string; labels: Label[]; selected?: number | null; onCreated: (label: Label) => void }) {
  const id = useId()
  const [value, setValue] = useState(selected ? String(selected) : '')
  const [adding, setAdding] = useState(false)
  const [draft, setDraft] = useState('')
  const input = useRef<HTMLInputElement>(null)
  const action = useAction()
  const title = kindNames[kind]
  const create = () => {
    const text = draft.trim()
    if (!text) { input.current?.focus(); return }
    if (action.busy) return
    void action.run(() => api<Label>(`${root}/labels`, 'POST', { kind, name: text, enabled: true }), label => { onCreated(label); setValue(String(label.id)); setAdding(false); setDraft('') })
  }
  return <div className="mb-4 grid min-w-0 gap-1.5 last:mb-0">
    <label htmlFor={id} className="text-sm leading-snug font-medium">{title}</label>
    <div className="flex gap-2">
      <NativeSelect id={id} name={name} className="w-full" value={value} onChange={event => setValue(event.target.value)}>
        <option value="">未设置</option>
        {labels.filter(label => label.kind === kind && (label.enabled || String(label.id) === value)).map(label => <option key={label.id} value={label.id}>{label.name}{label.enabled ? '' : '（已停用）'}</option>)}
      </NativeSelect>
      {!adding && <Button type="button" variant="outline" size="icon" aria-label={`新建${title}`} title={`新建${title}`} onClick={() => setAdding(true)}><PlusIcon aria-hidden="true" /></Button>}
    </div>
    {adding && <div className="flex gap-2">
      <Input ref={input} aria-label={`新的${title}名称`} placeholder={`新的${title}名称`} maxLength={100} autoFocus value={draft} onChange={event => setDraft(event.target.value)}
        onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); create() } else if (event.key === 'Escape') { event.preventDefault(); setAdding(false) } }} />
      <Button type="button" size="sm" onClick={create}>添加</Button>
      <Button type="button" variant="ghost" size="sm" onClick={() => setAdding(false)}>取消</Button>
    </div>}
    <ErrorNotice message={action.error} />
  </div>
}

/**
 * Adding or editing an item. What every item needs comes first; the rest folds
 * under 更多设置, which opens by itself when an edited item uses any of it.
 */
export default function ExpenseForm({ value, entries, labels, busy, stale, error, onSubmit, onClose }: { value: Expense | 'new'; entries: Expense[]; labels: Label[]; busy: boolean; stale: boolean; error: string; onSubmit: (form: FormData) => void; onClose: () => void }) {
  const entry = value === 'new' ? undefined : value
  const [added, setAdded] = useState<Label[]>([])
  const [auto, setAuto] = useState(entry?.auto_renew ?? true)
  const [notify, setNotify] = useState(entry?.notify ?? false)
  const all = [...labels, ...added.filter(label => !labels.some(existing => existing.id === label.id))]
  const created = (label: Label) => setAdded(current => [...current, label])
  const more = Boolean(entry && (entry.payment_method_id || entry.payer_id || entry.notify || entry.start_date || entry.cancellation_date || entry.replacement_id || entry.url || entry.notes || entry.inactive))
  return <FormDialog title={entry ? `编辑「${entry.name}」` : '添加项目'} onClose={onClose} onSubmit={onSubmit} busy={busy} submitDisabled={stale} error={error} submitLabel={entry ? '保存' : '添加'} slot="expense-form">
    <Field label="名称"><Input name="name" required maxLength={200} defaultValue={entry?.name ?? ''} placeholder="例如 ChatGPT Plus、example.com" autoComplete="off" /></Field>
    <div className="grid grid-cols-[minmax(0,1fr)_7rem] gap-3">
      <Field label="每期金额"><Input name="amount" required inputMode="decimal" defaultValue={entry ? decimal(entry.amount_units) : ''} placeholder="0.00" /></Field>
      <Field label="币种"><Input name="currency" required pattern="[A-Za-z]{3}" maxLength={3} list="expense-currencies" defaultValue={entry?.currency ?? 'CNY'} onBlur={event => { event.target.value = event.target.value.toUpperCase() }} /></Field>
    </div>
    <datalist id="expense-currencies">{currencies.map(code => <option key={code} value={code} />)}</datalist>
    <div className="grid grid-cols-2 gap-3">
      <div className="mb-4 grid min-w-0 gap-1.5">
        <span className="text-sm leading-snug font-medium">周期</span>
        <div className="flex items-center gap-2"><span className="text-muted-foreground text-sm">每</span><Input name="frequency" aria-label="每隔几个周期" required type="number" min={1} max={1000} defaultValue={entry?.frequency ?? 1} className="w-16" /><NativeSelect name="cycle" aria-label="周期单位" defaultValue={entry?.cycle ?? 'month'} className="min-w-0 flex-1">{Object.entries(cycles).map(([key, name]) => <option key={key} value={key}>{name}</option>)}</NativeSelect></div>
      </div>
      <Field label="下次续费" hint="改日期或周期会从这一天重新算起。"><Input name="next_payment" type="date" required defaultValue={entry?.next_payment ?? ''} /></Field>
    </div>
    <div className="mb-4 grid gap-1.5">
      <span className="text-sm leading-snug font-medium">续费方式</span>
      <SegmentedGroup aria-label="续费方式" className="w-full *:flex-1">
        <SegmentedButton aria-pressed={auto} onClick={() => setAuto(true)}>服务商自动扣款</SegmentedButton>
        <SegmentedButton aria-pressed={!auto} onClick={() => setAuto(false)}>我手动续费</SegmentedButton>
      </SegmentedGroup>
      <input type="hidden" name="auto_renew" value={auto ? 'on' : ''} />
      <small className="text-muted-foreground text-xs leading-relaxed">{auto ? '到日子自动进入下一期；需要记账时在详情里确认续期。' : '到日子后等你确认续期，确认时记一笔付款。'} apsp 不会发起任何扣款。</small>
    </div>
    <LabelSelect kind="category" name="category_id" labels={all} selected={entry?.category_id} onCreated={created} />
    <details data-slot="more-settings" open={more} className="group/more border-t pt-3">
      <summary className="text-muted-foreground hover:text-foreground flex cursor-pointer list-none items-center gap-1.5 text-sm font-medium [&::-webkit-details-marker]:hidden"><ChevronDownIcon className="size-4 transition-transform group-open/more:rotate-180" aria-hidden="true" />更多设置</summary>
      <div className="mt-3">
        <div className="grid gap-x-3 sm:grid-cols-2">
          <LabelSelect kind="payment_method" name="payment_method_id" labels={all} selected={entry?.payment_method_id} onCreated={created} />
          <LabelSelect kind="payer" name="payer_id" labels={all} selected={entry?.payer_id} onCreated={created} />
        </div>
        <div className="mb-4 flex flex-wrap items-center gap-3">
          <label className="flex cursor-pointer items-center gap-2.5 text-sm"><Switch name="notify" checked={notify} onCheckedChange={setNotify} />续费前提醒</label>
          <label className={`flex items-center gap-2 text-sm ${notify ? '' : 'text-muted-foreground'}`}>提前<Input name="notify_days_before" type="number" min={0} max={365} required defaultValue={entry?.notify_days_before ?? 3} className="w-16" aria-label="提前几天提醒" />天</label>
          <small className="text-muted-foreground basis-full text-xs">经「告警设置」里的渠道发送；在设置里可以一次关闭全部续费提醒。</small>
        </div>
        <div className="grid gap-x-3 sm:grid-cols-2">
          <Field label="开始日期"><Input name="start_date" type="date" defaultValue={entry?.start_date ?? ''} /></Field>
          <Field label="取消日期" hint="到这一天停用，之后不再续费。"><Input name="cancellation_date" type="date" defaultValue={entry?.cancellation_date ?? ''} /></Field>
        </div>
        <Field label="替代项目"><NativeSelect name="replacement_id" defaultValue={entry?.replacement_id ?? ''}><option value="">无</option>{entries.filter(other => other.id !== entry?.id).map(other => <option key={other.id} value={other.id}>{other.name}</option>)}</NativeSelect></Field>
        <Field label="管理网址"><Input name="url" type="url" maxLength={2048} defaultValue={entry?.url ?? ''} placeholder="https://" /></Field>
        <Field label="备注"><Textarea name="notes" maxLength={8000} rows={3} defaultValue={entry?.notes ?? ''} /></Field>
        <label className="flex cursor-pointer items-center gap-2.5 text-sm"><Switch name="inactive" defaultChecked={entry?.inactive ?? false} />停用（不计入费用，也不提醒）</label>
      </div>
    </details>
  </FormDialog>
}
