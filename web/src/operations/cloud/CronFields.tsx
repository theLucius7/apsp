import { useRef, useState } from 'react'
import { api, errorMessage } from '../../api'
import { ErrorNotice, Field } from '../../components'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

export function CronField({ label, value, offset, onChange }: { label: string; value: string; offset: number; onChange: (value: string) => void }) {
  const [result, setResult] = useState<{ key: string; times?: number[]; error?: string } | null>(null)
  const [busy, setBusy] = useState(false), generation = useRef(0)
  const key = `${value}:${offset}`, current = useRef(key)
  current.current = key
  const preview = async () => {
    const epoch = ++generation.current
    setBusy(true); setResult(null)
    try {
      const response = await api<{ occurrences: number[] }>('/api/resources/alicloud/cron-preview', 'POST', { expression: value, utc_offset_minutes: offset })
      if (epoch === generation.current && current.current === key) setResult({ key, times: response.occurrences })
    } catch (error) { if (epoch === generation.current && current.current === key) setResult({ key, error: errorMessage(error) }) }
    finally { if (epoch === generation.current) setBusy(false) }
  }
  return <div><div className="mb-4 flex items-end gap-2"><Field label={label} className="mb-0 min-w-0 flex-1"><Input required value={value} maxLength={256} placeholder="0 8 * * 1-5" onChange={event => onChange(event.target.value)} /></Field><Button type="button" variant="outline" disabled={busy || !value.trim()} onClick={() => void preview()} aria-label={`预览${label}`}>{busy ? '读取中…' : '预览'}</Button></div>
    {result?.key === key && <><ErrorNotice message={result.error ?? ''} />{result.times && <ol className="text-muted-foreground my-2 text-xs" aria-label={`${label}未来触发时间`}>{result.times.map(at => <li key={at}>{new Date((at + offset * 60) * 1000).toISOString().slice(0, 16).replace('T', ' ')}</li>)}</ol>}</>}
  </div>
}
