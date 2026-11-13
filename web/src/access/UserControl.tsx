import { api } from '../api'
import { ErrorNotice } from '../components'
import { resourceWriteError, useAction, useResource } from '../hooks'
import { Button } from '@/components/ui/button'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

type Control = { paused: boolean; revision: number }
export default function UserControl({ userId, active, onChanged }: { userId: number; active: boolean; onChanged: () => void }) {
  const url = `/api/access/users/${userId}/control`
  const control = useResource<Control>(active ? url : null, 5000, { keep: true })
  const action = useAction()
  const change = () => {
    if (resourceWriteError(control) || !control.getCurrent()) return
    const current = control.getCurrent()!
    void action.run(() => api(url, 'PUT', { expected_revision: current.revision, paused: !current.paused }), () => { control.reload(); onChanged() })
  }
  return <Panel className="mb-4"><PanelHeader><PanelTitle>使用状态</PanelTitle></PanelHeader><div className="p-4">
    <ErrorNotice message={control.error || action.error} retry={control.reload} />
    <p className="mb-3 text-sm">{control.data?.paused ? '已暂停。恢复后仍按原套餐额度与有效期判断资格。' : '未暂停。暂停后订阅立即停止提供节点，受管服务器应用新配置后停止连接。'}凭据、授权和历史用量保留。外部节点已下载的凭据需由提供方停用。</p>
    <Button variant={control.data?.paused ? 'default' : 'outline'} disabled={action.busy || Boolean(resourceWriteError(control))} onClick={change}>{control.data?.paused ? '恢复使用' : '暂停使用'}</Button>
  </div></Panel>
}
