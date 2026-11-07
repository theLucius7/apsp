import assert from 'node:assert/strict'

// Native option popups are outside the DOM's painted ancestor stack. Measure
// their own opaque colours, including options without a component class.
export async function assertNativeSelectContrast(page, theme) {
  const result = await page.evaluate(() => {
    const canvas = document.createElement('canvas')
    canvas.width = canvas.height = 1
    const context = canvas.getContext('2d', { willReadFrequently: true })
    const rgba = colour => {
      context.clearRect(0, 0, 1, 1)
      context.fillStyle = colour
      context.fillRect(0, 0, 1, 1)
      return [...context.getImageData(0, 0, 1, 1).data]
    }
    const luminance = colour => colour.slice(0, 3).reduce((sum, value, index) => {
      const channel = value / 255
      return sum + (channel <= .04045 ? channel / 12.92 : ((channel + .055) / 1.055) ** 2.4) * [.2126, .7152, .0722][index]
    }, 0)
    const controls = [...document.querySelectorAll('select')].filter(select => select.checkVisibility())
    const failures = []
    let checked = 0
    for (const select of controls) for (const option of select.querySelectorAll('option, optgroup')) {
      const style = getComputedStyle(option)
      const background = rgba(style.backgroundColor), foreground = rgba(style.color)
      const first = luminance(background), second = luminance(foreground)
      const ratio = (Math.max(first, second) + .05) / (Math.min(first, second) + .05)
      checked++
      if (background[3] !== 255 || foreground[3] !== 255 || ratio < 4.5) {
        failures.push({ text: option.label, background, foreground, ratio })
      }
    }
    return { checked, failures, schemes: controls.map(select => getComputedStyle(select).colorScheme) }
  })
  assert(result.checked > 0, 'The page must contain native options to check')
  assert.deepEqual(result.failures, [], `${theme} options need opaque surfaces and WCAG AA contrast`)
  assert(result.schemes.every(scheme => scheme === theme), 'Native controls must follow the application theme, even when the OS uses the opposite theme')
}
