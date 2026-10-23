import { useEffect, useRef, useState } from 'react'
import { api } from '../api'
import { agentInstallTargets, agentTargetLabel } from '../agent-installation'
import type { InstallationPlatform } from '../agent-installation'
import { Badge, CopyField, ErrorNotice, Field, Loading, Modal } from '../components'
import { navigate, time } from '../format'
import { useAction, useResource } from '../hooks'
import type { AgentVersions, Enrollment, Server } from '../types'
import { SetupSteps } from './ServerSetup'

import { Button, buttonVariants } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { NativeSelect } from '@/components/ui/native-select'
import { ActivityIcon, ArrowRightIcon, BoxIcon, CheckIcon, LockIcon, RefreshCwIcon, ServerIcon } from 'lucide-react'
import { Notice } from '@/components/panel'

export default function ServerEnrollment({ server, created = false, onClose }: { server: Server; created?: boolean; onClose: () => void }) {
  const action = useAction()
  const live = useResource<Server>(`/api/servers/${server.id}`, 3000)
  const catalog = useResource<AgentVersions>('/api/artifacts/agent-versions', 0)
  const [version, setVersion] = useState('')
  const [target, setTarget] = useState('')
  const [platform, setPlatform] = useState<InstallationPlatform>('unix')
  const filtered = useResource<AgentVersions>(`/api/artifacts/agent-versions?platform=${platform}${target ? `&target=${encodeURIComponent(target)}` : ''}`, 0)
  const versions = filtered.data?.versions ?? []
  const selectedVersion = versions.find(item => item.version === version)
  const versionsLoading = filtered.loading || (!filtered.data && !filtered.error)
  const versionsError = filtered.error || catalog.error
  const reloadVersions = () => { catalog.reload(); filtered.reload() }
  const availableTargets = agentInstallTargets(catalog.data?.versions.flatMap(item => item.targets) ?? [], platform)
  const [enrollment, setEnrollment] = useState<Enrollment | null>(null)
  const [now, setNow] = useState(Date.now())
  const [showHelp, setShowHelp] = useState(false)
  const generated = useRef(false)
  const generate = () => {
    const params = new URLSearchParams()
    if (version) params.set('agent_version', version)
    if (target) params.set('agent_target', target)
    if (platform === 'windows') params.set('platform', platform)
    const query = params.size ? `?${params}` : ''
    setEnrollment(null)
    void action.run(() => api<Enrollment>(`/api/servers/${server.id}/enrollment${query}`, 'POST'), setEnrollment)
  }
  useEffect(() => { if (!filtered.data || generated.current) return; generated.current = true; if (filtered.data.versions.length) generate() }, [filtered.data])
  useEffect(() => { const timer = window.setInterval(() => setNow(Date.now()), 1000); return () => window.clearInterval(timer) }, [])
  const entry = live.data ?? server
  const known = Boolean(live.data) && !live.error
  const online = known && entry.online
  const registered = Boolean(entry.device_public_key)
  const expired = enrollment != null && enrollment.expires_at * 1000 <= now
  const status = !known ? '正在确认设备状态' : online ? created ? '服务器已上线' : '设备当前在线' : registered ? '已注册，等待设备连接' : '等待设备接入'

  return <Modal title={created ? '接入服务器' : '接入 / 升级'} onClose={onClose} busy={action.busy} className="sm:max-w-[52.5rem]">
    <div className="bg-muted/40 px-4 py-5 sm:px-7 sm:py-6">
      {created && <SetupSteps step={2} />}
      <div className="flex items-center gap-3.5 py-6 max-sm:gap-3 max-sm:py-5"><span className="bg-primary/10 text-primary border-primary/20 grid size-12 flex-none place-items-center rounded-xl border max-sm:size-10">{created ? <CheckIcon className="size-6" aria-hidden="true" /> : <ServerIcon className="size-6" aria-hidden="true" />}</span><div className="min-w-0 flex-1"><h3 className="text-lg font-semibold tracking-tight wrap-anywhere max-sm:text-base">{server.name}</h3><p className="text-muted-foreground mt-1.5 text-xs leading-relaxed">{created ? '配置已保存。完成 Agent 安装后，监控与拨测会自动开始。' : '获取安装命令，重新接入或更新这台服务器上的 Agent。'}</p></div><Badge tone={online ? 'good' : 'neutral'}>{online ? '在线' : registered ? '已注册' : '待接入'}</Badge></div>
      <section className="bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4" aria-labelledby="enrollment-install">
        <div className="mb-4 flex items-center justify-between gap-3.5 max-sm:flex-wrap"><div className="min-w-0"><h3 className="flex items-center gap-2 text-sm font-semibold" id="enrollment-install">在服务器上安装 Agent</h3><p className="text-muted-foreground mt-1 text-xs leading-relaxed">{platform === 'windows' ? 'Windows · 在管理员 PowerShell 中执行' : 'Linux / macOS / FreeBSD · 在服务器终端执行'}</p></div><span className="bg-muted text-muted-foreground inline-block rounded border px-1.5 text-xs font-normal whitespace-nowrap">管理员权限</span></div>
        <form className="flex flex-wrap items-start gap-3 max-sm:flex-col" onSubmit={event => { event.preventDefault(); generate() }}>
          <Field className="flex-1 max-sm:w-full mb-0" label="服务器系统"><NativeSelect className="w-full" value={platform} onChange={event => { setPlatform(event.target.value as InstallationPlatform); setTarget(''); setVersion(''); setEnrollment(null); action.clearError() }} disabled={action.busy}>
            <option value="unix">Linux / macOS / FreeBSD（Shell）</option>
            <option value="windows">Windows（PowerShell）</option>
          </NativeSelect></Field>
          <Field className="flex-1 max-sm:w-full mb-0" label="目标系统与架构" hint="自动模式在执行时检测本机；手动选择须与执行机器一致。"><NativeSelect className="w-full" value={target} onChange={event => { setTarget(event.target.value); setVersion(''); setEnrollment(null); action.clearError() }} disabled={action.busy || catalog.loading}>
            <option value="">自动匹配系统与架构</option>
            {availableTargets.map(value => <option value={value} key={value}>{agentTargetLabel(value)}</option>)}
          </NativeSelect></Field>
          <Field className="flex-1 max-sm:w-full mb-0" label="Agent 版本" hint="列表只显示已收录、已验签且协议兼容的稳定版本。自动模式会在执行时选择本机最新兼容版本。"><NativeSelect className="w-full" value={version} onChange={event => { setVersion(event.target.value); setEnrollment(null); action.clearError() }} disabled={action.busy || versionsLoading || !versions.length}>
            <option value="">自动匹配最新兼容版本</option>
            {versions.map(item => <option value={item.version} key={item.version}>{item.version} · 已验签</option>)}
          </NativeSelect></Field>
          <Button type="submit" variant="outline" className="mt-6 max-sm:mt-0" disabled={action.busy || versionsLoading || !versions.length}><RefreshCwIcon aria-hidden="true" />{action.busy ? '正在生成…' : '重新生成命令'}</Button>
        </form>
        {selectedVersion && <p className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">所选版本可用于：{agentInstallTargets(selectedVersion.targets, platform).map(agentTargetLabel).join('、')}。</p>}
        <ErrorNotice message={versionsError} retry={reloadVersions} />
        {!versionsLoading && !versionsError && !versions.length && <p className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">当前没有适合此系统与架构的已签名版本，请维护者准备对应的 Agent 发布后重试。<a className="text-primary underline-offset-4 hover:underline" href="#/plugins/catalog" onClick={onClose}>查看已收录版本</a></p>}
        <ErrorNotice message={action.error} />
        {action.busy ? <Loading /> : enrollment ? expired ? <Notice role="status">接入令牌已过期，请重新生成命令。</Notice> : enrollment.install_command ? <div className="bg-muted/40 mt-5 overflow-hidden rounded-lg border">
          <div className="flex items-center justify-between gap-3 border-b px-4 py-2.5 max-sm:flex-wrap"><span className="text-foreground flex items-center gap-1.5 text-xs font-medium"><BoxIcon className="size-3.5" aria-hidden="true" />{version ? `签名版本 ${enrollment.installation?.version ?? version}` : '执行时自动匹配最新兼容版本'}</span><small className="text-muted-foreground text-xs">复制一行命令，在{platform === 'windows' ? '管理员 PowerShell' : '服务器终端'}粘贴执行</small></div>
          <div className="px-4"><CopyField text={enrollment.install_command} label="复制安装命令" singleLine /></div>
          <p className="text-muted-foreground px-4 pb-3 text-xs leading-relaxed">一次性接入令牌有效至 {time(enrollment.expires_at)}，请勿公开分享命令。</p>
        </div> : <Notice role="status"><span>{enrollment.warning ?? '请维护者准备对应的已签名 Agent 发布后重试。'}<br />服务器配置已保留，发布准备好后可在这里重新生成命令。</span></Notice> : !action.error && versions.length > 0 && <p className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">点击“重新生成命令”获取所选版本的接入命令。</p>}
        {action.error && <p className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">服务器已保存。重试只会生成新的接入命令，不会重复创建服务器。</p>}
        <div className="text-muted-foreground mt-4 flex gap-2"><LockIcon className="size-4 mt-0.5 flex-none" aria-hidden="true" /><p className="m-0 text-xs leading-relaxed">安装命令会自动下载安装器、准备验证工具，识别目标服务器的系统与架构，并验证对应的签名制品。升级保留设备身份与本地状态。</p></div>
        <p className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">安装时只下载当前系统与架构的 Agent；缺少兼容的签名版本时，会明确提示原因。</p>
      </section>
      <section className={cn('bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4', online && 'border-success/40 bg-success/5')} aria-labelledby="enrollment-status">
        <div className="mb-4 flex items-center justify-between gap-3.5 max-sm:flex-wrap"><div className="min-w-0"><h3 className="flex items-center gap-2 text-sm font-semibold" id="enrollment-status" aria-live="polite">{live.error ? '暂时无法确认设备状态' : status}</h3><p className="text-muted-foreground mt-1 text-xs leading-relaxed">{online ? `当前 Agent 版本：${entry.static_info.agent_version ?? '尚未上报'}。可进入详情查看采样与拨测结果。` : registered ? '设备身份已登记，等待 Agent 建立连接并上报状态。' : '执行命令后保持此页面打开，我们会自动检测连接状态。'}</p></div>{online ? <CheckIcon className="size-6 text-muted-foreground flex-none" aria-hidden="true" /> : <ActivityIcon className="size-6 text-muted-foreground flex-none" aria-hidden="true" />}</div>
        <ol className="m-0 flex list-none flex-wrap gap-x-4 gap-y-2 p-0">
          <li className="text-muted-foreground flex items-center gap-1.5 text-xs"><CheckIcon className="size-3.5" aria-hidden="true" />服务器已创建</li>
          <li className={cn('text-muted-foreground flex items-center gap-1.5 text-xs', registered && 'text-success')}>{registered ? <CheckIcon className="size-3.5" aria-hidden="true" /> : <LockIcon className="size-3.5" aria-hidden="true" />}{registered ? '设备已注册' : '等待设备注册'}</li>
          <li className={cn('text-muted-foreground flex items-center gap-1.5 text-xs', online && 'text-success')}>{online ? <CheckIcon className="size-3.5" aria-hidden="true" /> : <ActivityIcon className="size-3.5" aria-hidden="true" />}{online ? '设备在线' : '等待设备上线'}</li>
        </ol>
        <ErrorNotice message={live.error} retry={live.reload} />
        {online && <Notice><div><strong className="font-medium">下一步：安装服务器插件</strong><p className="text-muted-foreground mt-1 mb-3">监控已接入。安装 sing-box 后可创建代理节点、代理用户和两跳链路。</p><a className={buttonVariants({ size: 'sm' })} href="#/plugins/sing-box" onClick={onClose}>安装服务器插件<ArrowRightIcon className="size-3.5" aria-hidden="true" /></a></div></Notice>}
        <div className="mt-5 flex items-center justify-between gap-2.5 border-t pt-3"><span className="text-muted-foreground text-xs">页面可见时，每 3 秒自动检查</span><Button type="button" variant="ghost" size="sm" onClick={live.reload}>立即检查</Button></div>
        <Button type="button" variant="ghost" size="sm" className="mt-3.5" aria-expanded={showHelp} aria-controls="enrollment-help" onClick={() => setShowHelp(value => !value)}>{showHelp ? '收起排查说明' : '设备迟迟未上线？'}</Button>
        {showHelp && <ul id="enrollment-help" className="text-muted-foreground mt-3 list-disc pl-5 text-sm leading-relaxed"><li>确认安装命令执行成功，Agent 服务正在运行。</li><li>确认设备能访问命令中的面板地址，HTTPS 与 WebSocket 反向代理可用。</li><li>令牌过期或已被使用时，重新生成命令；已有服务器需保留原设备身份。</li><li>接入后暂时没有拨测结果时，等待配置同步，并检查目标地址与检测权限。</li></ul>}
      </section>
    </div>
    <footer className="bg-popover sticky bottom-0 z-[1] flex flex-wrap items-center justify-end gap-2 border-t px-4 py-3 sm:px-7"><span className="text-muted-foreground min-w-32 flex-1 text-xs max-sm:mb-1 max-sm:basis-full">关闭页面后仍可从详情继续接入</span><Button type="button" variant="outline" disabled={action.busy} onClick={onClose}>{online ? '完成' : '稍后接入'}</Button><Button type="button" disabled={action.busy} onClick={() => { onClose(); navigate(`/servers/${server.id}`) }}>查看服务器<ArrowRightIcon aria-hidden="true" /></Button></footer>
  </Modal>
}
