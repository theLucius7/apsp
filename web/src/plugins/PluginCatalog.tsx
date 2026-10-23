import { useState } from 'react'
import { Badge, CopyButton, Empty, ErrorNotice, Field, Icon, Loading, Modal, PageHeader, Refresh } from '../components'
import { bytes, navigate } from '../format'
import { useResource } from '../hooks'
import { pluginCatalog, pluginServerPath } from './catalog'
import type { CatalogDefinition, CatalogItem } from './catalog'
import type { Artifact, Server } from '../types'
import PluginCenterTabs from './PluginCenterTabs'
import './plugin-catalog.css'

function Packages({ item, unavailable }: { item: CatalogItem; unavailable: boolean }) {
  if (!item.versions.length) return <p className="catalog-package-note">{unavailable ? '暂时无法读取版本信息。' : '暂无已验证的下载包。插件介绍不代表服务器已经安装或可以执行。'}</p>
  return <details className="catalog-packages">
    <summary>查看版本与架构（{item.versions.length} 个版本）</summary>
    {unavailable && <p className="catalog-package-note">以下是上次读取的版本信息，当前分发状态未知。</p>}
    {item.versions.map(version => <section key={version.version} className="catalog-version" aria-label={`版本 ${version.version}`}>
      <h3>版本 <code>{version.version}</code></h3>
      <div className="table-wrap"><table><thead><tr><th>平台 / 架构</th><th>下载大小</th><th>校验摘要</th></tr></thead><tbody>
        {version.packages.map((artifact, index) => <tr key={`${artifact.arch}/${artifact.sha256}/${index}`}>
          <td><Badge>{artifact.arch}</Badge></td><td>{bytes(artifact.bytes)}</td>
          <td><div className="checksum"><code title={artifact.sha256}>{artifact.sha256.slice(0, 12)}…</code><CopyButton text={artifact.sha256} label="复制摘要" /></div></td>
        </tr>)}
      </tbody></table></div>
    </section>)}
  </details>
}

function SelectServer({ plugin, onClose }: { plugin: CatalogDefinition; onClose: () => void }) {
  const servers = useResource<Server[]>('/api/servers', 0)
  const [selected, setSelected] = useState('')
  const server = servers.data?.find(item => String(item.id) === selected)
  const path = server ? pluginServerPath(plugin, server.id) : null
  return <Modal title={`选择使用 ${plugin.title} 的服务器`} onClose={onClose}>
    <div className="modal-body">
      <p className="helper">{plugin.execution === 'panel' ? '选择服务器后启用 DDNS 插件，由面板使用 Agent 已上报的 IP 更新 DNS。' : '这里只选择要管理的服务器，不会立即安装或运行插件。后续操作由该服务器的 Agent 执行。'}</p>
      <ErrorNotice message={servers.error} retry={servers.reload} />
      {servers.loading && !servers.data ? <Loading /> : !servers.error && !servers.data?.length ? <Empty icon="server" title="还没有服务器" description="先添加服务器并接入 Agent，再使用插件。"><a className="button button-primary" href="#/servers">前往服务器</a></Empty> : <Field label="目标服务器">
        <select value={selected} onChange={event => setSelected(event.target.value)} disabled={!!servers.error}>
          <option value="">请选择服务器</option>
          {servers.data?.map(item => <option key={item.id} value={item.id}>{item.name}（{item.online ? '在线' : item.device_public_key ? '离线' : '待接入'}）</option>)}
        </select>
      </Field>}
    </div>
    <footer><button className="button button-secondary" onClick={onClose}>取消</button><button className="button button-primary" disabled={!path || !!servers.error} onClick={() => { if (path && !servers.error) { onClose(); navigate(path) } }}>前往服务器管理</button></footer>
  </Modal>
}

export default function PluginCatalog() {
  const resource = useResource<Artifact[]>('/api/artifacts', 0)
  const [selected, setSelected] = useState<CatalogDefinition | null>(null)
  const catalog = pluginCatalog(resource.data ?? [])
  const unavailable = !!resource.error || !resource.data
  return <>
    <PageHeader eyebrow="扩展" title="插件中心" description="先了解每个插件做什么、在哪里执行，再进入云账号或服务器管理。同一插件的各版本与架构集中展示。"><Refresh onClick={resource.reload} /></PageHeader>
    <PluginCenterTabs active="catalog" />
    <div className="notice quiet-notice"><Icon name="server" size={19} /><div><strong>按插件说明管理云账号或服务器</strong><p>设备插件由 Agent 下载、校验和执行；面板插件使用已上报的数据处理任务。下载包已验证，不代表服务器已安装或已就绪。</p></div></div>
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.loading && !resource.data && <Loading />}
    <div className="catalog-grid" aria-label="插件目录">
      {catalog.plugins.map(plugin => <article className="panel catalog-card" key={plugin.id} data-catalog-plugin={plugin.id}>
        <div className="panel-heading"><div className="catalog-identity"><Icon name={plugin.icon} /><h2>{plugin.title}</h2></div><Badge>{plugin.execution === 'panel' ? '面板插件' : unavailable ? '版本状态未知' : plugin.versions.length ? '有已验证下载包' : '暂无下载包'}</Badge></div>
        <div className="panel-body"><p className="catalog-description">{plugin.description}</p><p className="helper">{plugin.usage}</p>
          {!!plugin.architectures.length && <div className="catalog-architectures"><span>已收录架构</span>{plugin.architectures.map(arch => <Badge key={arch}>{arch}</Badge>)}</div>}
          {plugin.execution === 'panel' ? <p className="catalog-package-note">随面板提供，无需设备下载包。</p> : <Packages item={plugin} unavailable={unavailable} />}
        </div>
        <div className="catalog-actions"><span>执行位置：{plugin.execution === 'panel' ? '面板' : '服务器 Agent'}</span><button className="button button-secondary" onClick={() => plugin.panelPath ? navigate(plugin.panelPath) : setSelected(plugin)}>{plugin.panelPath ? '进入云服务管理' : '选择服务器'}<Icon name="arrow" size={15} /></button></div>
      </article>)}
    </div>
    <section className="panel catalog-agent" aria-label="基础组件"><div className="panel-heading"><h2>服务器 Agent</h2><Badge>基础组件，不是插件</Badge></div><div className="panel-body"><p className="catalog-description">采集服务器状态，接收面板配置，并管理服务器上的插件。接入或升级请在具体服务器页面操作。</p><Packages item={catalog.agent} unavailable={unavailable} /><a className="text-button" href="#/servers">管理服务器接入 <Icon name="arrow" size={15} /></a></div></section>
    {!!catalog.others.length && <section className="panel"><div className="panel-heading"><h2>其他分发组件</h2></div><div className="panel-body"><p className="helper">这些组件尚未登记插件介绍，仅展示下载包信息，不提供启用或执行入口。</p>{catalog.others.map(item => <div key={item.id} className="catalog-unknown"><h3>{item.id}</h3><Packages item={item} unavailable={unavailable} /></div>)}</div></section>}
    {selected && <SelectServer key={selected.id} plugin={selected} onClose={() => setSelected(null)} />}
  </>
}
