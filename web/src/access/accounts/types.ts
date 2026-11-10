export const root = '/api/plugins/sing-box'
export const publicRoot = '/api/plugins/sing-box/portal-public'
export type Content = { id: number; kind: string; title: string; category: string; body: string; published: boolean; version: number }
export type IssuedInvitation = { id: string; hint: string; origin: 'quota' | 'paid'; state: 'available' | 'claimed' | 'revoked'; created_at: number; claimed_at: number | null; revoked_at: number | null; recipient: { id: number; name: string } | null }
export const invitationStates: Record<IssuedInvitation['state'], string> = { available: '待使用', claimed: '已使用', revoked: '已撤销' }
export class RequestIdentity {
  private payload = ''
  private id = ''
  for(value: unknown): string { const payload = JSON.stringify(value); if (!this.id || payload !== this.payload) { this.payload = payload; this.id = crypto.randomUUID() } return this.id }
}
/**
 * Whether this page was opened at a user's own address (ADR 0103), where the sign-in
 * page lives at `/{path}/login`. That address answers only its owner's portal: the
 * email sign-in, recovery and invitation endpoints exist on the panel's address alone.
 */
export const isOwnAddressPath = (pathname: string) => /^\/[^/]+\/login\/?$/.test(pathname)
export const onOwnAddress = () => isOwnAddressPath(window.location.pathname)
