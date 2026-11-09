import type { Artifact } from '../types'

/** One distributed component: every verified version with the platforms that really exist for it. */
export type ArtifactVersion = { version: string; packages: Artifact[] }
export type ArtifactComponent = { name: string; title: string; versions: ArtifactVersion[]; architectures: string[] }
export type ArtifactGroup = { id: 'agent' | 'runtime' | 'diagnostics' | 'other'; title: string; components: ArtifactComponent[] }

// What each signed component is; the panel installs none of them from this page.
const components: readonly { name: string; title: string; group: ArtifactGroup['id'] }[] = [
  { name: 'agent', title: '服务器 Agent', group: 'agent' },
  { name: 'sing-box', title: 'sing-box 运行时', group: 'runtime' },
  { name: 'ipquality', title: 'IP 质量', group: 'diagnostics' },
  { name: 'tcpquality', title: 'TCP 连接诊断', group: 'diagnostics' },
  { name: 'nodequality', title: '服务器验机', group: 'diagnostics' },
]
const groups: readonly Omit<ArtifactGroup, 'components'>[] = [
  { id: 'agent', title: '服务器 Agent' },
  { id: 'runtime', title: '运行时' },
  { id: 'diagnostics', title: '诊断工具' },
  { id: 'other', title: '其他组件' },
]

export function artifactComponent(name: string, title: string, artifacts: readonly Artifact[]): ArtifactComponent {
  const versions = new Map<string, Artifact[]>()
  const architectures = new Set<string>()
  for (const artifact of artifacts) {
    if (artifact.name !== name) continue
    const packages = versions.get(artifact.version) ?? []
    packages.push(artifact)
    versions.set(artifact.version, packages)
    architectures.add(artifact.arch)
  }
  return {
    name,
    title,
    architectures: [...architectures].sort(),
    // Display order only: it does not choose a compatible or latest version.
    versions: [...versions].sort(([a], [b]) => b.localeCompare(a, 'en', { numeric: true })).map(([version, packages]) => ({
      version,
      packages: packages.sort((a, b) => a.arch.localeCompare(b.arch) || a.sha256.localeCompare(b.sha256)),
    })),
  }
}

/** Groups the verified artifacts by what they are; unknown components keep their own name. */
export function artifactGroups(artifacts: readonly Artifact[]): ArtifactGroup[] {
  const known = new Set(components.map(component => component.name))
  const others = [...new Set(artifacts.map(artifact => artifact.name))].filter(name => !known.has(name)).sort()
  return groups.map(group => ({
    ...group,
    components: group.id === 'other'
      ? others.map(name => artifactComponent(name, name, artifacts))
      : components.filter(component => component.group === group.id).map(component => artifactComponent(component.name, component.title, artifacts)),
  })).filter(group => group.id !== 'other' || group.components.length)
}
