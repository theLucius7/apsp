import { CopyField, ErrorNotice } from '../components'
import type { Enrollment } from '../types'

export default function InstallCommand({ enrollment }: { enrollment: Enrollment }) {
  return <><p className="helper">复制完整的一行命令，在目标服务器的终端粘贴执行。安装器会自动准备验证工具、识别系统与架构，验证并安装对应的签名制品；升级保留设备身份和本地状态。</p>{enrollment.install_command ? <><p className="helper">{enrollment.installation?.version === 'latest' ? '执行时自动匹配本机最新兼容版本' : `目标 Agent 版本：${enrollment.installation?.version}`}</p><CopyField text={enrollment.install_command} label="复制安装命令" /></> : <ErrorNotice message={enrollment.warning ?? '请维护者准备对应的已签名 Agent 发布后重试'} />}</>
}
