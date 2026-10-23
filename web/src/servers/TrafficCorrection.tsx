import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Field, Modal } from '../components'
import { useAction } from '../hooks'
import { assetDate, defaultAssets, parseTraffic, trafficUnits } from '../server-assets'
import type { TrafficUnit } from '../server-assets'
import type { Server } from '../types'

export default function TrafficCorrection({ server, onClose, onSaved }: { server: Server; onClose: () => void; onSaved: () => void }) {
  const traffic = server.traffic!, asset = { ...defaultAssets, ...server.asset_settings }
  const [up, setUp] = useState(traffic.uploaded), [down, setDown] = useState(traffic.downloaded), [unit, setUnit] = useState<TrafficUnit>('B'), [reason, setReason] = useState('')
  const action = useAction()
  return <Modal title="矫正本期流量" onClose={onClose} busy={action.busy}><form onSubmit={event => { event.preventDefault(); void action.run(() => api(`/api/servers/${server.id}/traffic-correction`, 'POST', {
    cycle_start: traffic.cycle_start, reset_day: asset.reset_day, network_interface: asset.network_interface, correction_id: traffic.correction_id ?? null,
    baseline_uploaded: traffic.uploaded, baseline_downloaded: traffic.downloaded, uploaded: parseTraffic(up, unit), downloaded: parseTraffic(down, unit), reason: reason.trim(),
  }), onSaved) }}><div className="modal-body"><fieldset disabled={action.busy}>
    <p>当前账单周期：{assetDate(traffic.cycle_start)} 至 {assetDate(traffic.cycle_end)}（UTC，不含结束日）。</p>
    <p>填写打开此窗口时应有的流量。保存期间新增的采样会继续累加，原始记录不变；下一周期自动失效。</p>
    <Field label="矫正后的上传流量"><input required inputMode="decimal" pattern="[0-9]+(\.[0-9]{1,6})?" value={up} onChange={event => setUp(event.target.value)} /></Field>
    <Field label="矫正后的下载流量"><input required inputMode="decimal" pattern="[0-9]+(\.[0-9]{1,6})?" value={down} onChange={event => setDown(event.target.value)} /></Field>
    <Field label="矫正流量单位" hint="切换单位后重新填写数值，默认字节可保留原有精度。"><select value={unit} onChange={event => { setUnit(event.target.value as TrafficUnit); setUp(''); setDown('') }}>{Object.keys(trafficUnits).map(unit => <option key={unit}>{unit}</option>)}</select></Field>
    <Field label="矫正原因"><input required maxLength={200} value={reason} onChange={event => setReason(event.target.value)} placeholder="例如：对齐供应商本期流量记录" /></Field>
  </fieldset><ErrorNotice message={action.error} /></div><footer><button type="button" className="button button-secondary" disabled={action.busy} onClick={onClose}>取消</button><button className="button button-primary" disabled={action.busy}>{action.busy ? '正在保存…' : '保存矫正'}</button></footer></form></Modal>
}
