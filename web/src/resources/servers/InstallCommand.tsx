import { CopyField, ErrorNotice } from '../../components'
import type { Enrollment } from '../../types'

export default function InstallCommand({ enrollment }: { enrollment: Enrollment }) {
  return <><p className="text-muted-foreground mt-3 text-xs leading-relaxed">在目标服务器终端粘贴执行这一行命令。安装器会识别系统与架构并验证签名；升级保留设备身份。</p>{enrollment.install_command ? <><p className="text-muted-foreground mt-3 text-xs leading-relaxed">{enrollment.installation?.version === 'latest' ? '执行时自动匹配本机最新兼容版本' : `目标 Agent 版本：${enrollment.installation?.version}`}</p><CopyField text={enrollment.install_command} label="复制安装命令" /></> : <ErrorNotice message={enrollment.warning ?? '请维护者准备对应的已签名 Agent 发布后重试'} />}</>
}
