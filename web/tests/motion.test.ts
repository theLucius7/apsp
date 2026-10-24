import { expect, test } from 'bun:test'
import { itemMotion, noticeMotion, pageMotion } from '../src/lib/motion'

// Readable content may move into place but never starts transparent or
// collapsed: if an animation cannot run, it must still be visible.
test('content entrance presets never hide their content', () => {
  for (const preset of [pageMotion, itemMotion, noticeMotion]) {
    expect(preset.initial).not.toHaveProperty('opacity')
    expect(preset.initial).not.toHaveProperty('height')
    expect(preset.initial).not.toHaveProperty('display')
  }
})
