import { useState } from 'react'
import { Badge, CopyButton, Empty, ErrorNotice, Field, Loading, Modal, PageHeader, Refresh } from '../components'
import { bytes, navigate } from '../format'
import { useResource } from '../hooks'
import { pluginCatalog, pluginServerPath } from '../plugins/catalog'
import type { CatalogDefinition, CatalogItem } from '../plugins/catalog'
import type { Artifact, Server } from '../types'

import { Button, buttonVariants } from '@/components/ui/button'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ArrowRightIcon, ServerIcon } from 'lucide-react'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'

function Packages({ item, unavailable }: { item: CatalogItem; unavailable: boolean }) {
  if (!item.versions.length) return <p className="text-muted-foreground my-4 text-xs leading-relaxed">{unavailable ? '暂时无法读取版本信息。' : '暂无已验证的下载包。插件介绍不代表服务器已经安装或可以执行。'}</p>
  return <details className="my-4 min-w-0">
    <summary className="focus-visible:ring-ring/50 cursor-pointer rounded-sm text-sm leading-relaxed outline-none focus-visible:ring-3">查看版本与架构（{item.versions.length} 个版本）</summary>
    {unavailable && <p className="text-muted-foreground my-4 text-xs leading-relaxed">以下是上次读取的版本信息，当前分发状态未知。</p>}
    {item.versions.map(version => <section data-slot="catalog-version" key={version.version} className="mt-4 min-w-0" aria-label={`版本 ${version.version}`}>
      <h3 className="mb-2.5 text-sm leading-relaxed font-semibold wrap-anywhere">版本 <code className="whitespace-normal">{version.version}</code></h3>
      <div className="max-w-full min-w-0"><Table className="min-w-[430px]"><TableHeader><TableRow><TableHead>平台 / 架构</TableHead><TableHead>下载大小</TableHead><TableHead>校验摘要</TableHead></TableRow></TableHeader><TableBody>
        {version.packages.map((artifact, index) => <TableRow key={`${artifact.arch}/${artifact.sha256}/${index}`}>
          <TableCell><Badge>{artifact.arch}</Badge></TableCell><TableCell>{bytes(artifact.bytes)}</TableCell>
          <TableCell><div className="flex items-center gap-3"><code className="whitespace-normal" title={artifact.sha256}>{artifact.sha256.slice(0, 12)}…</code><CopyButton text={artifact.sha256} label="复制摘要" /></div></TableCell>
        </TableRow>)}
      </TableBody></Table></div>
    </section>)}
  </details>
}

function SelectServer({ plugin, onClose }: { plugin: CatalogDefinition; onClose: () => void }) {
  const servers = useResource<Server[]>('/api/servers', 0)
  const [selected, setSelected] = useState('')
  const server = servers.data?.find(item => String(item.id) === selected)
  const path = server ? pluginServerPath(plugin, server.id) : null
  return <Modal title={`选择使用 ${plugin.title} 的服务器`} onClose={onClose}>
    <div data-slot="modal-body" className="p-4">
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{plugin.execution === 'panel' ? '选择服务器后启用 DDNS 插件，由面板使用 Agent 已上报的 IP 更新 DNS。' : '这里只选择要管理的服务器，不会立即安装或运行插件。后续操作由该服务器的 Agent 执行。'}</p>
      <ErrorNotice message={servers.error} retry={servers.reload} />
      {servers.loading && !servers.data ? <Loading /> : !servers.error && !servers.data?.length ? <Empty icon={ServerIcon} title="还没有服务器" description="先添加服务器并接入 Agent，再使用插件。"><a className={buttonVariants()} href="#/servers">前往服务器</a></Empty> : <Field label="目标服务器">
        <NativeSelect className="w-full" value={selected} onChange={event => setSelected(event.target.value)} disabled={!!servers.error}>
          <option value="">请选择服务器</option>
          {servers.data?.map(item => <option key={item.id} value={item.id}>{item.name}（{item.online ? '在线' : item.device_public_key ? '离线' : '待接入'}）</option>)}
        </NativeSelect>
      </Field>}
    </div>
    <footer><Button variant="outline" onClick={onClose}>取消</Button><Button disabled={!path || !!servers.error} onClick={() => { if (path && !servers.error) { onClose(); navigate(path) } }}>前往服务器管理</Button></footer>
  </Modal>
}

export default function PluginCatalog() {
  const resource = useResource<Artifact[]>('/api/artifacts', 0)
  const [selected, setSelected] = useState<CatalogDefinition | null>(null)
  const catalog = pluginCatalog(resource.data ?? [])
  const unavailable = !!resource.error || !resource.data
  return <>
    <PageHeader eyebrow="服务器扩展" title="插件目录" description="了解每个插件的用途，再进入账号或服务器管理。同一插件的不同版本与架构集中展示。"><Refresh onClick={resource.reload} /></PageHeader>
    <Notice tone="quiet"><ServerIcon className="text-muted-foreground mt-0.5 size-4 shrink-0" aria-hidden="true" /><div><strong className="font-medium">按插件说明管理云账号或服务器</strong><p className="text-muted-foreground">设备插件由 Agent 下载、校验和执行；面板插件使用已上报的数据处理任务。下载包已验证，不代表服务器已安装或已就绪。</p></div></Notice>
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.loading && !resource.data && <Loading />}
    <div data-slot="catalog-grid" className="mb-6 grid items-start gap-5 xl:grid-cols-2" aria-label="插件目录">
      {catalog.plugins.map(plugin => <article data-slot="panel" className="bg-card text-card-foreground ring-foreground/10 overflow-hidden rounded-xl ring-1 mb-0 min-w-0" key={plugin.id} data-catalog-plugin={plugin.id}>
        <PanelHeader><div className="flex min-w-0 items-center gap-2.5"><plugin.icon className="text-muted-foreground size-4" aria-hidden="true" /><PanelTitle className="wrap-anywhere">{plugin.title}</PanelTitle></div><Badge>{plugin.execution === 'panel' ? '面板插件' : unavailable ? '版本状态未知' : plugin.versions.length ? '有已验证下载包' : '暂无下载包'}</Badge></PanelHeader>
        <div className="p-4"><p className="text-foreground text-sm leading-relaxed">{plugin.description}</p><p className="text-muted-foreground mt-3 text-xs leading-relaxed">{plugin.usage}</p>
          {!!plugin.architectures.length && <div data-slot="catalog-architectures" className="my-4 flex flex-wrap items-center gap-2"><span className="text-muted-foreground text-xs">已收录架构</span>{plugin.architectures.map(arch => <Badge key={arch}>{arch}</Badge>)}</div>}
          {plugin.execution === 'panel' ? <p className="text-muted-foreground my-4 text-xs leading-relaxed">随面板提供，无需设备下载包。</p> : <Packages item={plugin} unavailable={unavailable} />}
        </div>
        <div className="flex flex-wrap items-center justify-between gap-3 border-t px-5 py-4"><span className="text-muted-foreground text-xs">执行位置：{plugin.execution === 'panel' ? '面板' : '服务器 Agent'}</span><Button variant="outline" onClick={() => plugin.panelPath ? navigate(plugin.panelPath) : setSelected(plugin)}>{plugin.panelPath ? '进入云服务管理' : '选择服务器'}<ArrowRightIcon aria-hidden="true" /></Button></div>
      </article>)}
    </div>
    <Panel aria-label="基础组件"><PanelHeader><PanelTitle>服务器 Agent</PanelTitle><Badge>基础组件，不是插件</Badge></PanelHeader><div className="p-4"><p className="text-foreground text-sm leading-relaxed">采集服务器状态，接收面板配置，并管理服务器上的插件。接入或升级请在具体服务器页面操作。</p><Packages item={catalog.agent} unavailable={unavailable} /><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href="#/servers">管理服务器接入 <ArrowRightIcon className="size-3.5" aria-hidden="true" /></a></div></Panel>
    {!!catalog.others.length && <Panel><PanelHeader><PanelTitle>其他分发组件</PanelTitle></PanelHeader><div className="p-4"><p className="text-muted-foreground mt-3 text-xs leading-relaxed">这些组件尚未登记插件介绍，仅展示下载包信息，不提供启用或执行入口。</p>{catalog.others.map(item => <div data-slot="catalog-unknown" key={item.id} className="mt-5 wrap-anywhere"><h3 className="text-sm font-semibold">{item.id}</h3><Packages item={item} unavailable={unavailable} /></div>)}</div></Panel>}
    {selected && <SelectServer key={selected.id} plugin={selected} onClose={() => setSelected(null)} />}
  </>
}
