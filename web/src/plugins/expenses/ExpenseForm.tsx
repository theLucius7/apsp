import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'
import { Checkbox } from '@/components/ui/checkbox'
import { Field, FormDialog } from '../../components'
import type { Expense, Label } from './types'
import { cycles, decimal } from './types'

export default function ExpenseForm({ value, entries, labels, busy, stale, error, onSubmit, onClose }: { value: Expense | 'new'; entries: Expense[]; labels: Label[]; busy: boolean; stale: boolean; error: string; onSubmit: (form: FormData) => void; onClose: () => void }) {
  const entry = value === 'new' ? undefined : value
  const label = (kind: Label['kind'], selected?: number | null) => <><option value="">未设置</option>{labels.filter(l => l.kind === kind && (l.enabled || l.id === selected)).map(l => <option key={l.id} value={l.id}>{l.name}{l.enabled ? '' : '（已停用）'}</option>)}</>
  return <FormDialog title={entry ? '编辑项目配置' : '添加项目'} onClose={onClose} onSubmit={onSubmit} busy={busy} submitDisabled={stale} error={error}>
    <Field label="名称"><Input name="name" required maxLength={200} defaultValue={entry?.name ?? ''} /></Field>
    <div className="grid gap-3 sm:grid-cols-2"><Field label="每期金额"><Input name="amount" required inputMode="decimal" defaultValue={entry ? decimal(entry.amount_units) : ''} placeholder="0.00" /></Field><Field label="币种"><Input name="currency" required pattern="[A-Z]{3}" maxLength={3} defaultValue={entry?.currency ?? 'CNY'} placeholder="CNY / USD" /></Field></div>
    <div className="grid gap-3 sm:grid-cols-2"><Field label="每隔"><Input name="frequency" required type="number" min={1} max={1000} defaultValue={entry?.frequency ?? 1} /></Field><Field label="周期"><NativeSelect name="cycle" defaultValue={entry?.cycle ?? 'month'}>{Object.entries(cycles).map(([key, name]) => <option key={key} value={key}>{name}</option>)}</NativeSelect></Field></div>
    <div className="grid gap-3 sm:grid-cols-2"><Field label="开始日期"><Input name="start_date" type="date" defaultValue={entry?.start_date ?? ''} /></Field><Field label="下次续费"><Input name="next_payment" type="date" required defaultValue={entry?.next_payment ?? ''} /></Field></div>
    <p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">按自然日历计算。月底项目保留原日号；修改下次续费或周期会重新设置周期起点。</p>
    <Field label="分类"><NativeSelect name="category_id" defaultValue={entry?.category_id ?? ''}>{label('category', entry?.category_id)}</NativeSelect></Field>
    <Field label="付款方式"><NativeSelect name="payment_method_id" defaultValue={entry?.payment_method_id ?? ''}>{label('payment_method', entry?.payment_method_id)}</NativeSelect></Field>
    <Field label="付款人"><NativeSelect name="payer_id" defaultValue={entry?.payer_id ?? ''}>{label('payer', entry?.payer_id)}</NativeSelect></Field>
    <Field label="管理网址"><Input name="url" type="url" maxLength={2048} defaultValue={entry?.url ?? ''} /></Field><Field label="备注"><Textarea name="notes" maxLength={8000} defaultValue={entry?.notes ?? ''} /></Field>
    <label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="auto_renew"  defaultChecked={entry?.auto_renew ?? true} />服务商自动续费</label><p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">apsp 仅记录和推进续费计划，不会发起扣款。</p>
    <label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="notify"  defaultChecked={entry?.notify ?? false} />启用此项目的续费提醒</label><Field label="提前提醒天数"><Input name="notify_days_before" type="number" min={0} max={365} required defaultValue={entry?.notify_days_before ?? 3} /></Field>
    <Field label="取消日期"><Input name="cancellation_date" type="date" defaultValue={entry?.cancellation_date ?? ''} /></Field><Field label="替代项目"><NativeSelect name="replacement_id" defaultValue={entry?.replacement_id ?? ''}><option value="">无</option>{entries.filter(e => e.id !== entry?.id).map(e => <option key={e.id} value={e.id}>{e.name}</option>)}</NativeSelect></Field>
    <label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="inactive"  defaultChecked={entry?.inactive ?? false} />已停用</label>
  </FormDialog>
}
