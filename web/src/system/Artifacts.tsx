import { Badge, CopyButton, ErrorNotice, Loading, PageHeader, Refresh, InfoTip } from '../components'
import { bytes } from '../format'
import { useResource } from '../hooks'
import { artifactGroups } from './artifactGroups'
import type { ArtifactComponent } from './artifactGroups'
import type { Artifact } from '../types'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

function Versions({ component, unavailable }: { component: ArtifactComponent; unavailable: boolean }) {
  if (!component.versions.length) return <p className="text-muted-foreground text-xs leading-relaxed">{unavailable ? '暂时无法读取版本信息。' : '暂无已验证的版本。'}</p>
  return <details className="min-w-0">
    <summary className="focus-visible:ring-ring/50 cursor-pointer rounded-sm text-sm leading-relaxed outline-none focus-visible:ring-3">{component.versions.length} 个版本 · 最新列出的是 <code>{component.versions[0].version}</code></summary>
    {unavailable && <p className="text-muted-foreground my-3 text-xs leading-relaxed">以下是上次读取的版本信息，当前状态未知。</p>}
    {component.versions.map(version => <section data-slot="artifact-version" key={version.version} className="mt-4 min-w-0" aria-label={`版本 ${version.version}`}>
      <h3 className="mb-2.5 text-sm leading-relaxed font-semibold wrap-anywhere">版本 <code className="whitespace-normal">{version.version}</code></h3>
      <div className="max-w-full min-w-0"><Table className="min-w-[430px]"><TableHeader><TableRow><TableHead>平台 / 架构</TableHead><TableHead>大小</TableHead><TableHead>校验摘要</TableHead></TableRow></TableHeader><TableBody>
        {version.packages.map((artifact, index) => <TableRow key={`${artifact.arch}/${artifact.sha256}/${index}`}>
          <TableCell><Badge>{artifact.arch}</Badge></TableCell><TableCell>{bytes(artifact.bytes)}</TableCell>
          <TableCell><div className="flex items-center gap-3"><code className="whitespace-normal" title={artifact.sha256}>{artifact.sha256.slice(0, 12)}…</code><CopyButton text={artifact.sha256} label="复制摘要" /></div></TableCell>
        </TableRow>)}
      </TableBody></Table></div>
    </section>)}
  </details>
}

/**
 * The signed components the panel trusts: the Agent, the runtime and the diagnostic
 * tools, each with its verified versions and platforms. Installing or running them
 * happens on the server they serve; this page only shows what is available.
 */
export default function Artifacts() {
  const resource = useResource<Artifact[]>('/api/artifacts', 0)
  const unavailable = !!resource.error || !resource.data
  const groups = artifactGroups(resource.data ?? [])
  return <>
    <PageHeader title="制品与版本"><Refresh onClick={resource.reload} /><InfoTip label="制品怎样使用"><p>这里是已导入并验签的发布。Agent 只从 GitHub Release 或配置的镜像下载并独立验签；运行时与诊断工具在各服务器上安装和执行。</p></InfoTip></PageHeader>
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.loading && !resource.data && <Loading variant="cards" rows={3} />}
    <div data-slot="artifact-groups" className="grid items-start gap-5 xl:grid-cols-2">
      {groups.map(group => <Panel key={group.id} data-artifact-group={group.id} className="mb-0 min-w-0">
        <PanelHeader><PanelTitle>{group.title}</PanelTitle></PanelHeader>
        <ul className="m-0 list-none p-0">
          {group.components.map(component => <li data-slot="artifact-component" data-artifact={component.name} key={component.name} className="min-w-0 border-b px-4 py-4 last:border-b-0">
            <div className="mb-2 flex flex-wrap items-center justify-between gap-2"><span className="text-sm font-medium wrap-anywhere">{component.title}</span>
              {!!component.architectures.length && <span data-slot="artifact-architectures" className="flex flex-wrap gap-1.5">{component.architectures.map(arch => <Badge key={arch}>{arch}</Badge>)}</span>}</div>
            <Versions component={component} unavailable={unavailable} />
          </li>)}
        </ul>
      </Panel>)}
    </div>
  </>
}
