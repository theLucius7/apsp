export const linuxAgentTarget = (target: string) => /^(?:(?:linux-(?:gnu|musl)-)?(?:amd64|arm64))$/.test(target)
export type InstallationPlatform = 'unix' | 'windows'

export function agentInstallTargets(targets: string[], platform: InstallationPlatform): string[] {
  const available = new Set<string>()
  for (const target of targets) {
    if (platform === 'windows') {
      if (/^windows-(?:amd64|arm64)$/.test(target)) available.add(target)
    } else if (linuxAgentTarget(target)) {
      const arch = target.endsWith('arm64') ? 'arm64' : 'amd64'
      available.add(`linux-gnu-${arch}`)
      if (!target.startsWith('linux-gnu-')) available.add(`linux-musl-${arch}`)
    } else if (/^(?:macos-arm64|freebsd-(?:amd64|arm64))$/.test(target)) available.add(target)
  }
  return [...available].sort()
}

const targetLabels: Record<string, string> = {
  'linux-gnu-amd64': 'Linux AMD64（GNU / glibc）',
  'linux-gnu-arm64': 'Linux ARM64（GNU / glibc）',
  'linux-musl-amd64': 'Linux AMD64（musl / Alpine）',
  'linux-musl-arm64': 'Linux ARM64（musl / Alpine）',
  amd64: 'Linux AMD64（静态制品）',
  arm64: 'Linux ARM64（静态制品）',
  'macos-arm64': 'macOS ARM64',
  'freebsd-amd64': 'FreeBSD AMD64',
  'freebsd-arm64': 'FreeBSD ARM64',
  'windows-amd64': 'Windows AMD64',
  'windows-arm64': 'Windows ARM64',
}
export const agentTargetLabel = (target: string) => targetLabels[target] ?? target
