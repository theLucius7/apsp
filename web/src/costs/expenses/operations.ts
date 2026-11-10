import { api } from '../../api'
import { itemInput, root } from './types'
import type { Expense } from './types'

/**
 * What can be done to a hand-added item, from the list or its detail. Only the
 * two that cannot be taken back from here ask first: confirming a renewal records
 * a payment, and deleting removes the item with its payment history. Stopping and
 * copying happen at once; the copy starts stopped, so nothing is reminded twice.
 */
export type ItemOperation = 'renew' | 'clone' | 'delete' | 'toggle'
export const confirmed = (kind: ItemOperation) => kind === 'renew' || kind === 'delete'

export const operationText = {
  renew: { title: (item: Expense) => `确认续期 · ${item.name}`, confirm: '确认续期', busy: '正在记录…',
    body: (item: Expense) => `${item.auto_renew ? '确认服务商已经扣款' : '确认已经续费'}：记一笔 ${item.next_payment} 起一期的付款，下次续费推进一个周期。付款记录不能在这里撤销。` },
  delete: { title: (item: Expense) => `删除「${item.name}」？`, confirm: '确认删除', busy: '正在删除…',
    body: () => '项目和它的付款记录会一起删除，尚未发送的提醒随之取消。只是不再续费的话，停用即可，记录会保留。' },
}

export async function runOperation(item: Expense, kind: ItemOperation, requestId: string): Promise<Expense | undefined> {
  if (kind === 'toggle') return api<Expense>(`${root}/subscriptions/${item.id}`, 'PUT', itemInput(item, { inactive: !item.inactive }))
  if (kind === 'delete') { await api(`${root}/subscriptions/${item.id}`, 'DELETE', { version: item.version }); return undefined }
  return api<Expense>(`${root}/subscriptions/${item.id}/${kind}`, 'POST', { version: item.version, request_id: requestId })
}
