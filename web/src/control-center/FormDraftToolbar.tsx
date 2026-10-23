import { useState } from 'react'
import { errorMessage } from '../api'
import { useFormDraft } from './preferences'
import { Button } from '@/components/ui/button'

export default function FormDraftToolbar<T>({ draftKey, value, dirty, onRestore, onSaved, description = '草稿按管理员独立保存；其他窗口修改后会提示冲突。', disabled = false }: { draftKey: string; value: T; dirty: boolean; onRestore: (value: T) => void; onSaved?: (value: T) => void; description?: string; disabled?: boolean }) {
  const draft = useFormDraft<T>(draftKey, dirty)
  const [busy, setBusy] = useState(false), [error, setError] = useState('')
  async function save() {
    setBusy(true); setError('')
    try { await draft.save(value); onSaved?.(value) } catch (failure) { setError(errorMessage(failure)) } finally { setBusy(false) }
  }
  return <div className="flex flex-wrap items-center gap-2.5">
    <Button variant="outline" type="button" disabled={disabled || busy || !draft.ready} onClick={() => void save()}>保存表单草稿</Button>
    <Button variant="outline" type="button" disabled={disabled || busy || !draft.ready || !draft.value} onClick={() => { if (draft.value && (!dirty || window.confirm('用已保存草稿替换当前输入？'))) onRestore(draft.value) }}>恢复表单草稿</Button>
    <Button variant="outline" type="button" disabled={busy} onClick={() => void draft.reload()}>读取最新草稿版本</Button>
    <small>{description}</small>{(error || draft.error) && <p className="text-destructive text-sm" role="alert">{error || draft.error}</p>}
  </div>
}
