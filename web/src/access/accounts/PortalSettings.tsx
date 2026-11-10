import { SegmentedButton, SegmentedNav } from '@/components/segmented'
import { useState } from 'react'
import { Modal } from '../../components'
import ContentEditor from './ContentEditor'
import InvitationSettings from './InvitationSettings'
import MailSettings from './MailSettings'
import SubscriptionDomain from '../SubscriptionDomain'

const tabs = ['邀请', '登录邮件', '公告与指南', '专属地址'] as const

/** Everything that shapes what proxy users see in their own portal; the owner also sets their own addresses here. */
export default function PortalSettings({ onClose, owner, onDomainSaved }: { onClose: () => void; owner: boolean; onDomainSaved: () => void }) {
  const [tab, setTab] = useState<(typeof tabs)[number]>('邀请')
  const shown = tabs.filter(entry => owner || entry !== '专属地址')
  return <Modal title="门户设置" onClose={onClose} wide>
    <div data-slot="modal-body" className="space-y-3 p-4">
      <SegmentedNav aria-label="门户设置">{shown.map(entry => <SegmentedButton key={entry} aria-pressed={tab === entry} onClick={() => setTab(entry)}>{entry}</SegmentedButton>)}</SegmentedNav>
      {tab === '邀请' && <InvitationSettings />}
      {tab === '登录邮件' && <MailSettings />}
      {tab === '公告与指南' && <ContentEditor />}
      {tab === '专属地址' && owner && <SubscriptionDomain onSaved={onDomainSaved} />}
    </div>
  </Modal>
}
