import { Badge } from '../components'
import { time } from '../format'
import type { DiagnosticRecord } from '../types'

const titles: Record<string, string> = {
  header_info: '报告信息', hardware_quality: '硬件质量', ip_quality: 'IP 质量',
  net_quality: '网络质量', backroute_trace: '回程路由',
  environment: '资源限制与开始负载', tcp_scope: 'TCP 测试范围', tcp_summary: 'TCP 连接汇总',
  ipquality_result: '节点出口查询与逐源结果',
}

export default function DiagnosticSections({ record }: { record: DiagnosticRecord }) {
  const chapters = record.sections ?? []
  const expected = record.expected_sections ?? []
  const complete = chapters.filter(chapter => chapter.complete).length
  const completeness = record.report_completeness ?? (record.report ? 'legacy' : 'empty')
  return <div className="diagnostic-sections">
    <p className="helper">报告完整度：{completeness === 'legacy' ? '历史报告，章节完整度未知' : completeness === 'complete' ? `完整 · ${complete} / ${expected.length} 章完成` : chapters.length ? `部分结果 · ${complete} / ${expected.length} 章完成` : '尚无章节结果'}。已保存的章节可以独立查看。</p>
    {chapters.map(chapter => <details key={chapter.name} className="quality-report-text">
      <summary>{titles[chapter.name] ?? chapter.name} <Badge tone={chapter.complete ? 'good' : 'warm'}>{chapter.complete ? '章节已完成' : '章节尚未完成'}</Badge></summary>
      <p className="helper">采集于 {time(chapter.collected_at)}</p><pre>{chapter.text}</pre>
    </details>)}
    {expected.filter(name => !chapters.some(chapter => chapter.name === name)).length > 0 && <p className="helper">尚无结果：{expected.filter(name => !chapters.some(chapter => chapter.name === name)).map(name => titles[name] ?? name).join('、')}。</p>}
  </div>
}
