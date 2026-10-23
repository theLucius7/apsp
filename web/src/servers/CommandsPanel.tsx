import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Loading } from '../components'
import { time } from '../format'
import { useAction, useResource } from '../hooks'

type State = 'queued' | 'claimed' | 'running' | 'cancel_requested' | 'cancelled' | 'succeeded' | 'failed' | 'expired' | 'interrupted'
type Control = { id: string; state: State; claimed_at: number | null; started_at: number | null; cancel_requested_at: number | null; finished_at: number | null; cancel_supported: boolean }
type Command = Omit<Control, 'id'> & {
  spec: { id: string; command: string; timeout_secs: number; expires_at: number }
  requested_at: number; lifecycle_version: number
  result: null | { status: State; stdout: string; stderr: string; finished_at: number; timed_out: boolean; truncated: boolean }
}
const labels: Record<State, string> = { queued: '等待领取', claimed: '设备已领取', running: '执行中', cancel_requested: '取消中，等待设备确认', cancelled: '已取消', succeeded: '执行成功', failed: '执行失败', expired: '领取已过期', interrupted: '执行被中断' }
const terminal = (state: State) => ['cancelled', 'succeeded', 'failed', 'expired', 'interrupted'].includes(state)

export default function CommandsPanel({ serverId, enabled }: { serverId: number; enabled: boolean }) {
  const base = `/api/servers/${serverId}/commands`
  const commands = useResource<Command[]>(base, 2000)
  const action = useAction(), cancellation = useAction()
  const [command, setCommand] = useState(''), [seconds, setSeconds] = useState(30), [ttl, setTtl] = useState(300)
  const [submitted, setSubmitted] = useState<Record<string, Control>>({})
  return <section className="panel"><div className="panel-heading"><h2>远程命令</h2><span className="subtle">设备服务账号执行</span></div>
    <div className="panel-body"><ErrorNotice message={commands.error || action.error || cancellation.error} />
      {!enabled && <p className="helper">远程命令默认关闭。需要在节点本机的 Agent 配置中启用 allow_remote_commands 并重启后才能使用。</p>}
      <form onSubmit={event => { event.preventDefault(); if (!enabled) return; void action.run(() => api(base, 'POST', { command, timeout_secs: seconds, ttl_secs: ttl }), () => { setCommand(''); commands.reload() }) }}>
        <label>命令<textarea required maxLength={16384} rows={4} value={command} onChange={event => setCommand(event.target.value)} placeholder="输入要在设备上执行的命令" /></label>
        <p className="helper">Unix 使用系统 Shell，Windows 使用 PowerShell。命令可能修改设备；执行中断后不会自动重试。</p>
        <div className="form-grid"><label>执行超时（秒）<input required type="number" min={1} max={600} value={seconds} onChange={event => setSeconds(Number(event.target.value))} /></label><label>设备领取期限（秒）<input required type="number" min={1} max={86400} value={ttl} onChange={event => setTtl(Number(event.target.value))} /></label></div>
        <button className="button button-primary" type="submit" disabled={action.busy || !enabled}>提交命令</button>
      </form>
    </div>
    <div className="panel-body">{commands.loading && !commands.data ? <Loading /> : commands.data?.length ? commands.data.map(item => {
      const current = terminal(item.state) ? item : submitted[item.spec.id] ?? item
      const cancellable = current.state === 'queued' || (['claimed', 'running'].includes(current.state) && item.lifecycle_version > 0 && item.cancel_supported)
      const started = item.started_at, finished = current.finished_at ?? item.result?.finished_at
      const elapsed = started == null ? null : Math.max(0, (finished ?? Math.floor(Date.now() / 1000)) - started)
      return <details key={item.spec.id}><summary>{labels[current.state]} · {time(item.requested_at)}</summary>
        <pre className="break-all">{item.spec.command}</pre>
        <p>领取时间：{item.claimed_at ? time(item.claimed_at) : '尚未领取'}<br />开始时间：{started ? time(started) : item.lifecycle_version === 0 && current.state !== 'queued' ? '旧设备未上报，无法确认' : '尚未开始'}<br />结束时间：{finished ? time(finished) : '尚未结束'}{elapsed != null && <><br />执行时间：{elapsed} 秒</>}</p>
        {cancellable && <button className="button button-secondary button-small" disabled={cancellation.busy} onClick={() => void cancellation.run(() => api<Control>(`${base}/${item.spec.id}/cancel`, 'POST'), control => { setSubmitted(previous => ({ ...previous, [item.spec.id]: control })); commands.reload() })}>{current.state === 'queued' ? '取消排队' : '取消执行'}</button>}
        {current.state === 'cancel_requested' && <p role="status">取消请求已保存，等待设备终止受管进程组并确认清理。设备离线时会持续等待；此时不能视为已停止。</p>}
        {cancellable && current.state !== 'queued' && <p className="helper">运行取消会终止本次命令的受管进程组；自行脱离该组的后台程序无法保证停止。</p>}
        {!terminal(current.state) && current.state !== 'queued' && !item.cancel_supported && <p className="helper">此设备不支持确认运行中取消，请等待执行结果或超时。更新 Agent 或更换支持的平台后可使用此能力。</p>}
        {current.state === 'interrupted' && <p>设备执行期间被中断，原命令不会自动重试。请查看结果中的恢复说明。</p>}
        {item.result && <><p>{item.result.timed_out && '已达到执行超时。'}{item.result.truncated && '输出超过上限，已截断。'}</p><h4>标准输出</h4><pre className="break-all">{item.result.stdout || '（无输出）'}</pre><h4>错误输出</h4><pre className="break-all">{item.result.stderr || '（无输出）'}</pre></>}
      </details>
    }) : <p className="subtle">尚未执行远程命令。</p>}</div>
  </section>
}
