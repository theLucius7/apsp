import { useState } from 'react'
import { errorMessage } from '../api'
import { ErrorNotice, PageHeader } from '../components'
import { requestReauthentication } from '../control-center/reauthentication-request'
import type { ActionRunner } from '../operations/types'
import BackupRecovery from './BackupRecovery'
import { Button } from '@/components/ui/button'

// Panel backups belong to the panel itself, so they live under 系统 rather
// than with the experimental cross-server automation.
export default function Backup() {
  const [version, setVersion] = useState(0)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  // Actions that need a recent proof ask for it through the shared dialog first.
  const run: ActionRunner = (action, proof = false) => {
    void (async () => {
      setBusy(true); setError('')
      try {
        if (proof) await requestReauthentication()
        await action()
      } catch (value) {
        setError(errorMessage(value))
      } finally {
        setBusy(false)
      }
    })()
  }
  return <div data-slot="backup-page" className="grid min-w-0 grid-cols-1 gap-4 *:min-w-0" aria-busy={busy}>
    <PageHeader eyebrow="系统" title="备份与恢复" description="登记面板的完整恢复点、按计划自动备份，并导入恢复演练报告。面板不能启动时，用独立恢复工具在本机恢复。">
      <Button variant="outline" disabled={busy} onClick={() => setVersion(value => value + 1)}>刷新状态</Button>
    </PageHeader>
    <ErrorNotice message={error} />
    <BackupRecovery run={run} version={version} />
  </div>
}
