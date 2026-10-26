import { api } from './api'

export type PasskeyInfo = { enabled: boolean; reason: string | null; origin: string }
export type PasskeyEntry = { id: string; name: string; created_at: number; last_used_at: number | null }
type Descriptor = Omit<PublicKeyCredentialDescriptor, 'id'> & { id: string }
type Creation = Omit<PublicKeyCredentialCreationOptions, 'challenge' | 'user' | 'excludeCredentials'> & {
  challenge: string; user: Omit<PublicKeyCredentialUserEntity, 'id'> & { id: string }; excludeCredentials?: Descriptor[]
}
type Request = Omit<PublicKeyCredentialRequestOptions, 'challenge' | 'allowCredentials'> & { challenge: string; allowCredentials?: Descriptor[] }
type Challenge<T> = { challenge_id: string; options: { publicKey: T } }

export function passkeySupport(): string {
  if (!window.isSecureContext) return 'Passkey 需要通过 HTTPS 访问；本地调试可使用 localhost。'
  if (!window.PublicKeyCredential || !navigator.credentials) return '当前浏览器不支持 Passkey，请使用支持通行密钥的浏览器。'
  return ''
}

function decode(value: string): ArrayBuffer {
  const text = atob(value.replace(/-/g, '+').replace(/_/g, '/'))
  const bytes = new Uint8Array(text.length)
  for (let i = 0; i < text.length; i++) bytes[i] = text.charCodeAt(i)
  return bytes.buffer
}

function encode(value: ArrayBuffer): string {
  return btoa(String.fromCharCode(...new Uint8Array(value))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

async function credential(operation: () => Promise<Credential | null>): Promise<PublicKeyCredential> {
  try {
    const value = await operation()
    if (!(value instanceof PublicKeyCredential)) throw new Error('没有收到 Passkey 验证结果，请重新开始。')
    return value
  } catch (error) {
    if (error instanceof DOMException) {
      if (['NotAllowedError', 'AbortError'].includes(error.name)) throw new Error('Passkey 操作已取消或超时，请重新尝试。')
      if (error.name === 'InvalidStateError') throw new Error('此认证器已绑定，请选择另一把 Passkey。')
      if (error.name === 'SecurityError') throw new Error('当前访问地址不能使用此 Passkey，请使用面板配置的公开地址。')
      if (error.name === 'NotSupportedError') throw new Error('此认证器不支持所需的验证方式，请更换设备或浏览器。')
    }
    throw error
  }
}

export async function registerPasskey(base: string, body: unknown, admin = true): Promise<void> {
  const support = passkeySupport()
  if (support) throw new Error(support)
  const challenge = await api<Challenge<Creation>>(`${base}/register/start`, 'POST', body, undefined, admin)
  const options = challenge.options.publicKey
  const key = await credential(() => navigator.credentials.create({ publicKey: {
    ...options, challenge: decode(options.challenge), user: { ...options.user, id: decode(options.user.id) },
    excludeCredentials: options.excludeCredentials?.map(item => ({ ...item, id: decode(item.id) })),
  } }))
  const response = key.response as AuthenticatorAttestationResponse
  await api(`${base}/register/finish`, 'POST', { challenge_id: challenge.challenge_id, credential: {
    id: key.id, rawId: encode(key.rawId), type: key.type, extensions: key.getClientExtensionResults(),
    response: { attestationObject: encode(response.attestationObject), clientDataJSON: encode(response.clientDataJSON), transports: response.getTransports?.() ?? [] },
  } }, undefined, admin)
}

export async function authenticatePasskey(base: string, admin = true): Promise<void> {
  const support = passkeySupport()
  if (support) throw new Error(support)
  const challenge = await api<Challenge<Request>>(`${base}/start`, 'POST', undefined, undefined, admin)
  const options = challenge.options.publicKey
  const key = await credential(() => navigator.credentials.get({ publicKey: {
    ...options, challenge: decode(options.challenge), allowCredentials: options.allowCredentials?.map(item => ({ ...item, id: decode(item.id) })),
  } }))
  const response = key.response as AuthenticatorAssertionResponse
  await api(`${base}/finish`, 'POST', { challenge_id: challenge.challenge_id, credential: {
    id: key.id, rawId: encode(key.rawId), type: key.type, extensions: key.getClientExtensionResults(),
    response: { authenticatorData: encode(response.authenticatorData), clientDataJSON: encode(response.clientDataJSON), signature: encode(response.signature), userHandle: response.userHandle ? encode(response.userHandle) : null },
  } }, undefined, admin)
}

export { authenticatePasskey as loginPasskey }
