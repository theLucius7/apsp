import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'
import { Checkbox } from '@/components/ui/checkbox'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { useState } from 'react'
import { api } from '../../../api'
import { ErrorNotice, Field, FormDialog } from '../../../components'
import { useAction, useResource } from '../../../hooks'
import { root } from './types'
import type { Content } from './types'

export default function ContentEditor() {
  const content = useResource<{ entries: Content[] }>(`${root}/content`), action = useAction()
  const [editing, setEditing] = useState<Content | 'new' | null>(null)
  const stale = !content.isCurrent() || Boolean(content.error)
  const submit = (form: FormData) => {
    if (!editing || stale) return
    void action.run(() => api(editing === 'new' ? `${root}/content` : `${root}/content/${editing.id}`, editing === 'new' ? 'POST' : 'PUT', { kind: form.get('kind'), title: form.get('title'), category: form.get('category'), body: form.get('body'), published: form.get('published') === 'on', version: editing === 'new' ? null : editing.version }), () => { setEditing(null); content.reload() })
  }
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>公告与指南</PanelTitle><Button size="sm" disabled={stale} onClick={() => setEditing('new')}>新增内容</Button></div><ErrorNotice message={content.error} retry={content.reload} />
    <div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>标题</TableHead><TableHead>分类</TableHead><TableHead>状态</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{content.data?.entries.map(c => <TableRow key={c.id}><TableCell>{c.title}</TableCell><TableCell>{c.kind === 'notice' ? '公告' : c.category || '指南'}</TableCell><TableCell>{c.published ? '已发布' : '草稿'}</TableCell><TableCell><Button variant="ghost" size="sm" disabled={stale} onClick={() => { action.clearError(); setEditing(c) }}>编辑</Button></TableCell></TableRow>)}</TableBody></Table></div>
    {editing && <FormDialog title={editing === 'new' ? '新增内容' : '编辑内容'} onClose={() => setEditing(null)} onSubmit={submit} busy={action.busy} submitDisabled={stale} error={action.error}>
      <Field label="类型"><NativeSelect name="kind" defaultValue={editing === 'new' ? 'notice' : editing.kind}><option value="notice">公告</option><option value="guide">指南</option></NativeSelect></Field>
      <Field label="标题"><Input name="title" required maxLength={200} defaultValue={editing === 'new' ? '' : editing.title} /></Field><Field label="分类"><Input name="category" maxLength={100} defaultValue={editing === 'new' ? '' : editing.category} /></Field>
      <Field label="正文"><Textarea className="min-h-60" name="body" required maxLength={100000} defaultValue={editing === 'new' ? '' : editing.body} /></Field><p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">正文按安全文本展示，保留段落和链接文字。</p>
      <label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="published"  defaultChecked={editing !== 'new' && editing.published} />向成员发布</label>
    </FormDialog>}
  </Panel>
}
