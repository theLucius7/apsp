import { Panel } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from '../../../api'
import { Brand, ErrorNotice, Field, Loading } from '../../../components'
import { useAction } from '../../../hooks'
import { RequestIdentity } from './types'

export default function Join({ token }: { token: string }) {
  const [data, setData] = useState<{ available: boolean; prepaid: boolean; package_name: string | null; email_verification_required: boolean; configuration: { enabled: boolean; reason?: string } }>(), [error, setError] = useState('')
  const [email, setEmail] = useState(''), [challenge, setChallenge] = useState(''), [notice, setNotice] = useState('')
  const action = useAction(), identity = useRef(new RequestIdentity())
  useEffect(() => { const c = new AbortController(); setData(undefined); setChallenge(''); void api<typeof data>('/api/plugins/sing-box/commerce-public/invitation', 'POST', { token }, c.signal, false).then(setData).catch(e => { if (!c.signal.aborted) setError(errorMessage(e)) }); return () => c.abort() }, [token])
  return <main className="mx-auto max-w-[880px] px-6 pt-7 pb-16 max-sm:px-3 max-sm:py-5 [&>header]:mb-7 [&>header]:flex [&>header]:items-center [&>header]:gap-4"><header><Brand /><span>成员邀请</span></header><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><h1>加入成员服务</h1></div><div className="p-4 space-y-3"><ErrorNotice message={error || action.error} />{notice && <p role="status">{notice}</p>}{!data && !error ? <Loading /> : data?.available ? <form onSubmit={e => {
    e.preventDefault(); const form = new FormData(e.currentTarget), payload = { token, name: form.get('name'), email }
    void action.run(() => api<{ url: string }>('/api/plugins/sing-box/commerce-public/claim', 'POST', { ...payload, request_id: identity.current.for(payload), challenge_id: challenge || null, code: form.get('code') || null }, undefined, false), result => { const url = new URL(result.url); if (url.origin !== window.location.origin) throw new Error('请从面板配置的公开地址重新打开邀请。'); window.location.assign(url.toString()) })
  }}><p>{data.prepaid ? `邀请人已预付「${data.package_name}」，领取后开通。` : '此邀请提供成员准入，首次套餐由邀请人为你购买。'}</p><Field label="名称"><Input name="name" required maxLength={128} autoComplete="nickname" /></Field><Field label="联系邮箱"><Input name="email" required type="email" maxLength={254} autoComplete="email" value={email} onChange={e => { setEmail(e.target.value); setChallenge(''); setNotice('') }} /></Field>
    {data.email_verification_required && <><Button type="button" variant="ghost" size="sm" disabled={action.busy || !email} onClick={() => void action.run(() => api<{ challenge_id: string; message: string }>('/api/plugins/sing-box/commerce-public/account/challenge', 'POST', { email, purpose: 'invite', invitation: token }, undefined, false), value => { setChallenge(value.challenge_id); setNotice(value.message) })}>发送邮箱验证码</Button><Field label="六位验证码"><Input name="code" required pattern="[0-9]{6}" inputMode="numeric" autoComplete="one-time-code" /></Field></>}
    <p>领取后绑定 Passkey，作为以后访问账户的登录方式。</p>{!data.configuration.enabled && <p className="my-3 rounded-lg border bg-muted/50 p-3 text-sm">{data.configuration.reason}</p>}<Button  disabled={action.busy || !data.configuration.enabled || (data.email_verification_required && !challenge)}>领取并设置登录方式</Button>
  </form> : data ? <p>邀请已使用、撤销或暂不可用。请联系邀请人。</p> : null}</div></Panel></main>
}
