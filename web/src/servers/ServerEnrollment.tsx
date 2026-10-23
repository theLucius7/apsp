import { useEffect, useRef, useState } from 'react'
import { api } from '../api'
import { agentInstallTargets, agentTargetLabel } from '../agent-installation'
import type { InstallationPlatform } from '../agent-installation'
import { Badge, CopyField, ErrorNotice, Field, Icon, Loading, Modal } from '../components'
import { navigate, time } from '../format'
import { useAction, useResource } from '../hooks'
import type { AgentVersions, Enrollment, Server } from '../types'
import { SetupSteps } from './ServerSetup'
import './server-setup.css'

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

  return <Modal title={created ? '接入服务器' : '接入 / 升级'} onClose={onClose} busy={action.busy} className="server-setup-modal">
    <div className="server-setup-body">
      {created && <SetupSteps step={2} />}
      <div className="server-setup-intro"><span className="server-setup-mark"><Icon name={created ? 'check' : 'server'} size={25} /></span><div><h3>{server.name}</h3><p>{created ? '配置已保存。完成 Agent 安装后，监控与拨测会自动开始。' : '获取安装命令，重新接入或更新这台服务器上的 Agent。'}</p></div><Badge tone={online ? 'good' : 'neutral'}>{online ? '在线' : registered ? '已注册' : '待接入'}</Badge></div>
      <section className="server-setup-section" aria-labelledby="enrollment-install">
        <div className="server-setup-heading"><div><h3 id="enrollment-install">在服务器上安装 Agent</h3><p>{platform === 'windows' ? 'Windows · 在管理员 PowerShell 中执行' : 'Linux / macOS / FreeBSD · 在服务器终端执行'}</p></div><span className="server-setup-tag">管理员权限</span></div>
        <form className="server-enrollment-version" onSubmit={event => { event.preventDefault(); generate() }}>
          <Field label="服务器系统"><select value={platform} onChange={event => { setPlatform(event.target.value as InstallationPlatform); setTarget(''); setVersion(''); setEnrollment(null); action.clearError() }} disabled={action.busy}>
            <option value="unix">Linux / macOS / FreeBSD（Shell）</option>
            <option value="windows">Windows（PowerShell）</option>
          </select></Field>
          <Field label="目标系统与架构" hint="自动模式在执行时检测本机；手动选择须与执行机器一致。"><select value={target} onChange={event => { setTarget(event.target.value); setVersion(''); setEnrollment(null); action.clearError() }} disabled={action.busy || catalog.loading}>
            <option value="">自动匹配系统与架构</option>
            {availableTargets.map(value => <option value={value} key={value}>{agentTargetLabel(value)}</option>)}
          </select></Field>
          <Field label="Agent 版本" hint="列表只显示已收录、已验签且协议兼容的稳定版本。自动模式会在执行时选择本机最新兼容版本。"><select value={version} onChange={event => { setVersion(event.target.value); setEnrollment(null); action.clearError() }} disabled={action.busy || versionsLoading || !versions.length}>
            <option value="">自动匹配最新兼容版本</option>
            {versions.map(item => <option value={item.version} key={item.version}>{item.version} · 已验签</option>)}
          </select></Field>
          <button type="submit" className="button button-secondary" disabled={action.busy || versionsLoading || !versions.length}><Icon name="refresh" size={15} />{action.busy ? '正在生成…' : '重新生成命令'}</button>
        </form>
        {selectedVersion && <p className="server-setup-help">所选版本可用于：{agentInstallTargets(selectedVersion.targets, platform).map(agentTargetLabel).join('、')}。</p>}
        <ErrorNotice message={versionsError} retry={reloadVersions} />
        {!versionsLoading && !versionsError && !versions.length && <p className="server-setup-help">当前没有适合此系统与架构的已签名版本，请维护者准备对应的 Agent 发布后重试。<a href="#/plugins/catalog" onClick={onClose}>查看已收录版本</a></p>}
        <ErrorNotice message={action.error} />
        {action.busy ? <Loading /> : enrollment ? expired ? <div className="notice" role="status">接入令牌已过期，请重新生成命令。</div> : enrollment.install_command ? <div className="server-enrollment-command">
          <div><span><Icon name="box" size={15} />{version ? `签名版本 ${enrollment.installation?.version ?? version}` : '执行时自动匹配最新兼容版本'}</span><small>复制一行命令，在{platform === 'windows' ? '管理员 PowerShell' : '服务器终端'}粘贴执行</small></div>
          <CopyField text={enrollment.install_command} label="复制安装命令" />
          <p>一次性接入令牌有效至 {time(enrollment.expires_at)}，请勿公开分享命令。</p>
        </div> : <div className="notice" role="status"><span>{enrollment.warning ?? '请维护者准备对应的已签名 Agent 发布后重试。'}<br />服务器配置已保留，发布准备好后可在这里重新生成命令。</span></div> : !action.error && versions.length > 0 && <p className="server-setup-help">点击“重新生成命令”获取所选版本的接入命令。</p>}
        {action.error && <p className="server-setup-help">服务器已保存。重试只会生成新的接入命令，不会重复创建服务器。</p>}
        <div className="server-enrollment-trust"><Icon name="lock" size={17} /><p>安装命令会自动下载安装器、准备验证工具，识别目标服务器的系统与架构，并验证对应的签名制品。升级保留设备身份与本地状态。</p></div>
        <p className="server-setup-help">安装时只下载当前系统与架构的 Agent；缺少兼容的签名版本时，会明确提示原因。</p>
      </section>
      <section className={`server-setup-section server-enrollment-status ${online ? 'is-online' : ''}`} aria-labelledby="enrollment-status">
        <div className="server-setup-heading"><div><h3 id="enrollment-status" aria-live="polite">{live.error ? '暂时无法确认设备状态' : status}</h3><p>{online ? `当前 Agent 版本：${entry.static_info.agent_version ?? '尚未上报'}。可进入详情查看采样与拨测结果。` : registered ? '设备身份已登记，等待 Agent 建立连接并上报状态。' : '执行命令后保持此页面打开，我们会自动检测连接状态。'}</p></div>{online ? <Icon name="check" size={23} /> : <Icon name="activity" size={23} />}</div>
        <ol className="server-enrollment-checks">
          <li className="is-done"><Icon name="check" size={14} />服务器已创建</li>
          <li className={registered ? 'is-done' : ''}><Icon name={registered ? 'check' : 'lock'} size={14} />{registered ? '设备已注册' : '等待设备注册'}</li>
          <li className={online ? 'is-done' : ''}><Icon name={online ? 'check' : 'activity'} size={14} />{online ? '设备在线' : '等待设备上线'}</li>
        </ol>
        <ErrorNotice message={live.error} retry={live.reload} />
        {online && <div className="notice"><div><strong>下一步：按需启用插件</strong><p>监控已接入，这台服务器已经可以在看板、告警和运维工具中使用。需要提供代理时，启用 sing-box 后即可创建代理节点、代理用户和链路。</p><a className="button button-primary button-small" href="#/plugins/sing-box" onClick={onClose}>前往代理服务<Icon name="arrow" size={15} /></a></div></div>}
        <div className="server-enrollment-status-actions"><span>页面可见时，每 3 秒自动检查</span><button type="button" className="text-button" onClick={live.reload}>立即检查</button></div>
        <button type="button" className="text-button server-enrollment-help-toggle" aria-expanded={showHelp} aria-controls="enrollment-help" onClick={() => setShowHelp(value => !value)}>{showHelp ? '收起排查说明' : '设备迟迟未上线？'}</button>
        {showHelp && <ul id="enrollment-help" className="server-enrollment-help"><li>确认安装命令执行成功，Agent 服务正在运行。</li><li>确认设备能访问命令中的面板地址，HTTPS 与 WebSocket 反向代理可用。</li><li>令牌过期或已被使用时，重新生成命令；已有服务器需保留原设备身份。</li><li>接入后暂时没有拨测结果时，等待配置同步，并检查目标地址与检测权限。</li></ul>}
      </section>
    </div>
    <footer className="server-setup-footer"><span>关闭页面后仍可从详情继续接入</span><button type="button" className="button button-secondary" disabled={action.busy} onClick={onClose}>{online ? '完成' : '稍后接入'}</button><button type="button" className="button button-primary" disabled={action.busy} onClick={() => { onClose(); navigate(`/servers/${server.id}`) }}>查看服务器<Icon name="arrow" size={16} /></button></footer>
  </Modal>
}
