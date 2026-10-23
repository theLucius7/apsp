import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from '../api'
import { Field } from '../components'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'

import type { ReauthenticationRequest } from './reauthentication-request'
export default function Reauthentication() {
  const [pending, setPending] = useState<ReauthenticationRequest[]>([])
  const requests = useRef<ReauthenticationRequest[]>([])
  const [password, setPassword] = useState('')
  const [code, setCode] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  useEffect(() => {
    const request = (event: Event) => {
      requests.current = [...requests.current, (event as CustomEvent<ReauthenticationRequest>).detail]
      setPending(requests.current)
    }
    window.addEventListener('apsp:reauthentication', request)
    return () => {
      window.removeEventListener('apsp:reauthentication', request)
      for (const item of requests.current) item.reject(new Error('管理员会话已结束，操作尚未提交。'))
      requests.current = []
    }
  }, [])
  if (!pending.length) return null
  const close = () => {
    for (const item of requests.current) item.reject(new Error('已取消再次验证，操作尚未提交。'))
    requests.current = []
    setPending([]); setPassword(''); setCode(''); setError('')
  }
  return <Dialog open onOpenChange={open => { if (!open && !busy) close() }}>
    <DialogContent showCloseButton={false} onInteractOutside={event => event.preventDefault()} onEscapeKeyDown={event => { if (busy) event.preventDefault() }} className="z-[60] sm:max-w-md">
      <DialogHeader>
        <DialogTitle>再次验证身份</DialogTitle>
        <DialogDescription>此操作需要验证当前管理员密码；启用二步验证时还需验证码。证明在当前会话内有效五分钟。</DialogDescription>
      </DialogHeader>
      <form className="grid" onSubmit={event => {
        event.preventDefault(); if (busy) return
        setBusy(true); setError('')
        void api('/api/control-center/reauth', 'POST', { password, totp_code: code || null }).then(() => {
          const accepted = requests.current
          requests.current = []
          for (const item of accepted) item.resolve()
          setPending([]); setPassword(''); setCode('')
        }).catch(error => setError(errorMessage(error))).finally(() => setBusy(false))
      }}>
        <Field label="当前管理员密码"><Input type="password" autoComplete="current-password" autoFocus required value={password} onChange={event => setPassword(event.target.value)} disabled={busy} /></Field>
        <Field label="二步验证码"><Input inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" maxLength={6} value={code} onChange={event => setCode(event.target.value)} disabled={busy} /></Field>
        {error && <p role="alert" className="text-destructive mb-3 text-sm">{error}</p>}
        <DialogFooter>
          <Button variant="outline" type="button" disabled={busy} onClick={close}>取消操作</Button>
          <Button type="submit" disabled={busy}>{busy ? '正在验证…' : '验证并继续'}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
}
