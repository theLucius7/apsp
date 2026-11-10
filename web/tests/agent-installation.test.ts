import { expect, test } from 'bun:test'
import { agentInstallTargets, linuxAgentTarget } from '../src/resources/agent-installation'

test('ARM static Agent packages offer both compatible Linux hosts without AMD', () => {
  expect(agentInstallTargets(['arm64', 'linux-musl-arm64'], 'unix')).toEqual(['linux-gnu-arm64', 'linux-musl-arm64'])
})

test('GNU Agent packages cannot be offered to musl hosts', () => {
  expect(agentInstallTargets(['linux-gnu-amd64'], 'unix')).toEqual(['linux-gnu-amd64'])
})

test('native targets are offered only to their matching command platform', () => {
  const targets = ['macos-arm64', 'freebsd-amd64', 'windows-arm64', 'riscv64']
  expect(agentInstallTargets(targets, 'unix')).toEqual(['freebsd-amd64', 'macos-arm64'])
  expect(agentInstallTargets(targets, 'windows')).toEqual(['windows-arm64'])
  expect(linuxAgentTarget('linux-gnu-arm64')).toBe(true)
  expect(linuxAgentTarget('arm64')).toBe(true)
  expect(linuxAgentTarget('macos-arm64')).toBe(false)
})
