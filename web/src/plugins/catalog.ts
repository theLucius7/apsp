import { CloudCogIcon, GaugeIcon, GlobeIcon, NetworkIcon, WaypointsIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { Artifact } from '../types'

export type CatalogDefinition = {
  id: string
  title: string
  description: string
  usage: string
  icon: LucideIcon
  serverSection?: '/plugins' | '/node-quality' | '/tcp-quality' | '/ddns'
  execution?: 'panel'
  panelPath?: string
}

// Product identities are independent of release versions and platform packages.
export const pluginDefinitions: readonly CatalogDefinition[] = [
  {
    id: 'alicloud', title: '阿里云 CDT 与带宽', icon: CloudCogIcon, execution: 'panel', panelPath: '/plugins/alicloud',
    description: '查看阿里云 CDT 国内、海外用量与账单，调整 ECS 固定公网 IP 和独立 EIP 的带宽。',
    usage: '按云账号登记资源，预览并确认后提交变配。自动降速默认关闭，需要同时启用账号与资源策略；无需 Agent 安装。',
  },
  {
    id: 'ddns', title: '动态域名解析', icon: GlobeIcon, serverSection: '/ddns', execution: 'panel',
    description: '使用服务器 Agent 上报的公网 IP，自动更新 Cloudflare、腾讯云、阿里云和华为云的 A / AAAA 记录。',
    usage: '按服务器启用后配置域名与云服务凭据。DNS 同步由面板插件执行，无需额外设备安装包；停用时保留现有解析。',
  },
  {
    id: 'sing-box', title: 'sing-box', icon: WaypointsIcon, serverSection: '/plugins',
    description: '在服务器上提供代理节点，管理代理用户、订阅、策略组、套餐和用量周期。',
    usage: '先为目标服务器启用并安装插件，等待 Agent 确认安装与运行状态，再创建节点和分配代理用户。',
  },
  {
    id: 'nodequality', title: 'NodeQuality', icon: GaugeIcon, serverSection: '/node-quality',
    description: '从目标服务器检查网络与系统状况，查看分段报告和历史诊断结果。',
    usage: '在服务器的节点诊断页面选择检查项目。完整验机仍受安全门禁限制，是否可执行以该服务器显示的状态为准。',
  },
  {
    id: 'tcpquality', title: 'TCP 连接诊断', icon: NetworkIcon, serverSection: '/tcp-quality',
    description: '从目标服务器向自有或获准使用的目标发起 TCP 连接，比较连接成功率和耗时。',
    usage: '在服务器上配置拨测目标后使用。这不是带宽测速，也不把连接失败率解释为网络丢包率；执行前仍需通过设备能力和安全检查。',
  },
]

export type CatalogVersion = { version: string; packages: Artifact[] }
export type CatalogItem = { id: string; versions: CatalogVersion[]; architectures: string[] }

export function catalogItem(id: string, artifacts: readonly Artifact[]): CatalogItem {
  const versions = new Map<string, Artifact[]>()
  const architectures = new Set<string>()
  for (const artifact of artifacts) {
    if (artifact.name !== id) continue
    const packages = versions.get(artifact.version) ?? []
    packages.push(artifact)
    versions.set(artifact.version, packages)
    architectures.add(artifact.arch)
  }
  return {
    id,
    architectures: [...architectures].sort(),
    // This is display ordering, not a compatibility or latest-version selector.
    versions: [...versions].sort(([a], [b]) => b.localeCompare(a, 'en', { numeric: true })).map(([version, packages]) => ({
      version,
      packages: packages.sort((a, b) => a.arch.localeCompare(b.arch) || a.sha256.localeCompare(b.sha256)),
    })),
  }
}

export function pluginCatalog(artifacts: readonly Artifact[]) {
  const known = new Set(['agent', ...pluginDefinitions.map(plugin => plugin.id)])
  return {
    plugins: pluginDefinitions.map(plugin => ({ ...plugin, ...catalogItem(plugin.id, artifacts) })),
    agent: catalogItem('agent', artifacts),
    others: [...new Set(artifacts.map(artifact => artifact.name))].filter(name => !known.has(name)).sort().map(name => catalogItem(name, artifacts)),
  }
}

export function isCatalogPath(path: string) { return path === '/plugins/catalog' || path === '/artifacts' }

export function pluginServerPath(plugin: CatalogDefinition, serverId: number): string | null {
  if (!plugin.serverSection || !Number.isSafeInteger(serverId) || serverId <= 0) return null
  return `/servers/${serverId}${plugin.serverSection}`
}
