import { Panel } from '@/components/panel'
import { useSystemTheme } from '@/lib/use-system-theme'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from '../../../api'
import { Brand, ErrorNotice, Field, Loading } from '../../../components'
import { useAction } from '../../../hooks'
import { publicRoot, RequestIdentity } from './types'

type Inspection = { available: boolean; package_name: string | null; email_verification_required: boolean; configuration: { enabled: boolean; reason?: string } }

/** Redeems an invitation code: the visitor creates their own account with it. */
export default function Join({ token }: { token?: string }) {
  useSystemTheme()
  const [code, setCode] = useState(token ?? '')
  const [checked, setChecked] = useState(token ?? '')
  const [data, setData] = useState<Inspection>(), [error, setError] = useState('')
  const [email, setEmail] = useState(''), [challenge, setChallenge] = useState(''), [notice, setNotice] = useState('')
  const action = useAction(), identity = useRef(new RequestIdentity())
  useEffect(() => {
    if (!checked) return
    const c = new AbortController(); setData(undefined); setError(''); setChallenge('')
    void api<Inspection>(`${publicRoot}/invitation`, 'POST', { token: checked }, c.signal, false).then(setData).catch(e => { if (!c.signal.aborted) setError(errorMessage(e).includes('不存在') || errorMessage(e).includes('404') ? '邀请码不存在，请核对后重新输入。' : errorMessage(e)) })
    return () => c.abort()
  }, [checked])
  return <main className="mx-auto max-w-[560px] px-6 pt-7 pb-16 max-sm:px-3 max-sm:py-5"><header className="mb-7 flex items-center gap-4"><Brand /><span className="text-muted-foreground text-sm">接受邀请</span></header><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><h1 className="m-0 text-base font-semibold">用邀请码创建账号</h1></div><div className="space-y-3 p-4"><ErrorNotice message={error || action.error} />{notice && <p className="text-sm" role="status">{notice}</p>}
    {!checked || error ? <form onSubmit={e => { e.preventDefault(); setChecked(code.trim()) }}><Field label="邀请码"><Input required autoComplete="off" autoCapitalize="characters" spellCheck={false} placeholder="XXXX-XXXX-XXXX-XXXX" value={code} onChange={e => setCode(e.target.value)} /></Field><Button disabled={!code.trim()}>下一步</Button></form>
      : !data ? <Loading /> : data.available ? <form onSubmit={e => {
        e.preventDefault(); const form = new FormData(e.currentTarget), payload = { token: checked, name: form.get('name'), email }
        void action.run(() => api<{ url: string }>(`${publicRoot}/claim`, 'POST', { ...payload, request_id: identity.current.for(payload), challenge_id: challenge || null, code: form.get('code') || null }, undefined, false), result => { const url = new URL(result.url); if (url.origin !== window.location.origin) throw new Error('请从面板配置的公开地址重新打开邀请。'); window.location.assign(url.toString()) })
      }}>
        <p className="text-sm">{data.package_name ? `创建后即可使用「${data.package_name}」。` : '创建后，管理员为你分配套餐即可使用。'}</p>
        <Field label="名称"><Input name="name" required maxLength={128} autoComplete="nickname" /></Field>
        <Field label="联系邮箱"><Input name="email" required type="email" maxLength={254} autoComplete="email" value={email} onChange={e => { setEmail(e.target.value); setChallenge(''); setNotice('') }} /></Field>
        {data.email_verification_required && <><Button type="button" variant="ghost" size="sm" disabled={action.busy || !email} onClick={() => void action.run(() => api<{ challenge_id: string; message: string }>(`${publicRoot}/account/challenge`, 'POST', { email, purpose: 'invite', invitation: checked }, undefined, false), value => { setChallenge(value.challenge_id); setNotice(value.message) })}>发送邮箱验证码</Button><Field label="六位验证码"><Input name="code" required pattern="[0-9]{6}" inputMode="numeric" autoComplete="one-time-code" /></Field></>}
        <p className="text-muted-foreground text-xs">创建后绑定 Passkey，作为以后的登录方式。</p>{!data.configuration.enabled && <p className="bg-muted/50 my-3 rounded-lg border p-3 text-sm">{data.configuration.reason}</p>}
        <Button disabled={action.busy || !data.configuration.enabled || (data.email_verification_required && !challenge)}>创建账号</Button>
      </form> : <><p className="text-sm">这个邀请码已使用、已撤销或暂不可用，请联系邀请你的人。</p><Button variant="outline" onClick={() => { setChecked(''); setData(undefined) }}>换一个邀请码</Button></>}
  </div></Panel></main>
}
