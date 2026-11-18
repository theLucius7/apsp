import { Badge, Empty, ErrorNotice, InfoTip, Loading, PageHeader, Refresh } from '../components'
import { useResource } from '../hooks'
import type { AgentVersions } from '../types'
import { agentTargetLabel } from './agent-installation'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { buttonVariants } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { PackageIcon } from 'lucide-react'

const selectionLabels: Record<string, string> = {
  highest_stable_signed_protocol_compatible_for_target: '按目标平台选择最高已签名且协议兼容的稳定版本',
}

/**
 * Signed self-contained Agent releases and the panel's selection policy. Each Agent
 * release bundles the sing-box runtime and diagnostic capabilities; devices verify
 * signatures themselves when enrolling or upgrading.
 */
export default function AgentUpdates() {
  const resource = useResource<AgentVersions>('/api/system/agent-releases', 0)
  const policy = resource.data?.policy
  const versions = resource.data?.versions ?? []
  return <>
    <PageHeader title="Agent 版本">
      <Refresh onClick={resource.reload} />
      <InfoTip label="Agent 发布与升级方式">
        <p>一体化 Agent 已内置 sing-box 运行时与线路诊断能力，无需单独选择或部署组件版本。设备从外部可信发布源或已配置的下载加速地址拉取发布包并独立验签，升级时保留设备身份。</p>
      </InfoTip>
      <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href="#/servers">前往服务器列表</a>
    </PageHeader>
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.loading && !resource.data && <Loading variant="cards" rows={3} />}
    {policy && <Panel data-slot="agent-release-policy">
      <PanelHeader><PanelTitle>发布与升级策略</PanelTitle></PanelHeader>
      <div className="grid gap-3 p-4">
        <dl className="m-0 grid gap-3 md:grid-cols-3">
          <div className="bg-muted/40 grid min-w-0 gap-1 rounded-lg border px-3.5 py-2.5">
            <dt className="text-muted-foreground text-xs">默认版本策略</dt>
            <dd className="m-0 text-sm font-medium wrap-anywhere">{policy.default_version === 'latest' ? '自动匹配最新兼容版本' : policy.default_version} <code className="text-muted-foreground text-xs">{policy.default_version}</code></dd>
          </div>
          <div className="bg-muted/40 grid min-w-0 gap-1 rounded-lg border px-3.5 py-2.5">
            <dt className="text-muted-foreground text-xs">版本匹配规则</dt>
            <dd className="m-0 text-sm font-medium wrap-anywhere">{selectionLabels[policy.selection] ?? policy.selection} <code className="text-muted-foreground block text-xs">{policy.selection}</code></dd>
          </div>
          <div className="bg-muted/40 grid min-w-0 gap-1 rounded-lg border px-3.5 py-2.5">
            <dt className="text-muted-foreground text-xs">最低允许接入版本</dt>
            <dd className="m-0 font-mono text-sm font-semibold tabular-nums">{policy.minimum_version}</dd>
          </div>
        </dl>
        <p className="text-muted-foreground text-xs leading-relaxed">新服务器接入或手工更新请在服务器详情点击「接入 / 升级」生成命令；已接入服务器可在各自的「Agent 与任务」中开启自动更新。具体服务器的内置组件版本在服务器详情中直接读取。</p>
      </div>
    </Panel>}
    <Panel data-slot="agent-releases">
      <PanelHeader>
        <PanelTitle>已收录签名版本</PanelTitle>
        {resource.data && <span className="text-muted-foreground text-xs">共 {versions.length} 个版本</span>}
      </PanelHeader>
      {!resource.loading && !versions.length ? !resource.error && <Empty icon={PackageIcon} title="暂无已收录的签名版本" description="维护者准备并收录官方签名 Agent 发布后，接入与自动升级可用的版本会在此列出。"><a className={buttonVariants({ variant: 'outline' })} href="#/servers">查看服务器</a></Empty> : versions.length > 0 && <div className="max-w-full min-w-0">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>版本号</TableHead>
              <TableHead>发布标签</TableHead>
              <TableHead>支持平台与架构</TableHead>
              <TableHead>协议范围</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {versions.map(item => <TableRow data-slot="agent-release" data-agent-version={item.version} key={item.version}>
              <TableCell><strong className="font-mono text-sm">{item.version}</strong></TableCell>
              <TableCell><code className="text-xs wrap-anywhere">{item.tag}</code></TableCell>
              <TableCell>
                <div className="flex flex-wrap gap-1.5">
                  {[...item.targets].sort().map(target => <Badge key={target}><span title={target}>{agentTargetLabel(target)}</span></Badge>)}
                </div>
              </TableCell>
              <TableCell className="font-mono text-xs tabular-nums">
                {item.protocol_min === item.protocol_max ? `v${item.protocol_min}` : `v${item.protocol_min}–v${item.protocol_max}`}
              </TableCell>
            </TableRow>)}
          </TableBody>
        </Table>
      </div>}
    </Panel>
  </>
}
