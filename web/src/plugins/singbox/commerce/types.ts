export const root = '/api/plugins/sing-box/commerce'
export type Settings = { enabled: boolean; invitations_enabled: boolean; invite_price_cents: number; version: number }
export type Member = { user_id: number; name: string; email: string | null; state: string; version: number; balance_cents: number; inviter_id: number | null; sponsor_required: boolean; notes?: string; account_id?: string }
export type PackageSnapshot = { name: string; monthly_bytes: string | null; duration_days: number; timezone: string; reset_day: number; reset_hour: number; reset_minute: number }
export type Offer = { id: number; name: string; description: string; package_group_id: number; package_snapshot: PackageSnapshot; policy_group_ids: number[]; price_cents: number; duration_months: number | null; enabled: boolean; renewable: boolean; capacity: number | null; version: number }
export type Ledger = { id: number; action: string; delta_cents: number; balance_after: number; reason: string; created_at: number }
export type Order = { id: string; payer_id: number; beneficiary_id: number; snapshot: Offer; amount_cents: number; state: string; created_at: number }
export type Invitation = { id: string; issuer_id: number; state: string; snapshot: Offer | null; admission_cents: number; prepaid_cents: number; recipient_id: number | null; created_at: number }
export type Channel = { id: number; name: string; gateway: string; merchant: string; credential_id: string; payment_type: string; fixed_fee_cents: number; fee_basis_points: number; enabled: boolean; version: number }
export type Topup = { id: string; user_id: number; channel_id: number; amount_cents: number; fee_cents: number; state: string; created_at: number }
export type Content = { id: number; kind: string; title: string; category: string; body: string; published: boolean; version: number }
export type History = { ledger: Ledger[]; orders: Order[]; invitations: Invitation[]; topups: Topup[]; legacy: { kind: string; source_id: string; record: unknown }[] }
export type MemberDetail = { profile: Member; history: History; events: { id: number; action: string; created_at: number }[] }
export type PaymentException = { id: number; topup_id: string; code: string; amount_cents: number; provider_reference: string; user_id: number }
export function money(cents: number | string): string { return `¥${(Number(cents) / 100).toFixed(2)}` }
export function parseMoney(value: FormDataEntryValue | string | null): number {
  const text = String(value ?? '').trim()
  if (!/^-?\d{1,8}(?:\.\d{1,2})?$/.test(text)) throw new Error('金额最多两位小数。')
  const negative = text.startsWith('-'), [whole, fraction = ''] = text.replace('-', '').split('.')
  const cents = Number(whole) * 100 + Number(fraction.padEnd(2, '0'))
  return negative ? -cents : cents
}
export function paymentFee(cents: number, channel: Pick<Channel, 'fixed_fee_cents' | 'fee_basis_points'>): number { return channel.fixed_fee_cents + Math.floor((cents * channel.fee_basis_points + 5000) / 10000) }
export const statuses: Record<string, string> = { active: '正常', pending: '待处理', suspended: '停用', archived: '归档', staged: '待迁移', available: '待领取', claimed: '已领取', revoked: '已撤销', fulfilled: '已交付', refunded: '已退款', paid: '已入账', cancelled: '已取消', expired: '已过期', exception: '待核对', credited: '已入账' }
export const status = (value: string) => statuses[value] ?? value
export class RequestIdentity {
  private payload = ''
  private id = ''
  for(value: unknown): string { const payload = JSON.stringify(value); if (!this.id || payload !== this.payload) { this.payload = payload; this.id = crypto.randomUUID() } return this.id }
}
