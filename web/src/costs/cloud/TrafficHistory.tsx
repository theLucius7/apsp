import { useState } from 'react'
import { ErrorNotice, Modal } from '../../components'
import { useResource } from '../../hooks'
import Chart from '../../observability/display/Chart'
import { Button } from '@/components/ui/button'
import { trafficPoints } from './usageHistory'
import type { TrafficHistory as History } from './usageHistory'
import type { Account } from '../../resources/cloud/types'

export function TrafficHistory({ account, close }: { account: Account; close: () => void }) {
  const result = useResource<History>(`/api/plugins/alicloud/accounts/${account.id}/traffic-history`, 60000)
  const [seconds, setSeconds] = useState(3600), [delta, setDelta] = useState(false)
  const group = result.data?.groups.find(group => group.seconds === seconds)
  return <Modal title={`CDT 用量历史 · ${account.name}`} onClose={close}>
    <div data-slot="modal-body" className="grid gap-4 p-4">
      <div className="flex flex-wrap gap-2" role="group" aria-label="历史范围">
        {[3600, 86400].map(value => <Button key={value} variant={seconds === value ? 'default' : 'outline'} onClick={() => setSeconds(value)} aria-pressed={seconds === value}>{value === 3600 ? '24 小时' : '30 天'}</Button>)}
      </div>
      <div className="flex flex-wrap gap-2" role="group" aria-label="用量口径">
        {[false, true].map(value => <Button key={String(value)} variant={delta === value ? 'default' : 'outline'} onClick={() => setDelta(value)} aria-pressed={delta === value}>{value ? '期间增量' : '当月累计'}</Button>)}
      </div>
      <ErrorNotice message={result.error} />
      {result.loading && <p>正在读取用量历史…</p>}
      {group && <Chart title="CDT 用量" from={group.from * 1000} to={group.to * 1000} gap={group.seconds * 1500} format={value => value === null ? '暂无采样' : `${value.toFixed(2)} GiB`} series={[
        { label: '中国内地', color: 'var(--chart-1)', points: trafficPoints(group, 'mainland_bytes', delta) },
        { label: '海外', color: 'var(--chart-2)', points: trafficPoints(group, 'overseas_bytes', delta) },
      ]} />}
      <p className="text-muted-foreground text-xs">按账号额度池记录 CDT 总用量，已包含 IPv4 和 IPv6。每日按北京时间归档，图表时间按浏览器时区显示；缺失采样保留空白。跨月、用量回退或缺少相邻采样时不计算增量。当前小时／天尚未结束，数据保留 32 天。</p>
    </div>
    <footer><Button variant="outline" onClick={result.reload}>刷新历史</Button><Button onClick={close}>关闭</Button></footer>
  </Modal>
}
