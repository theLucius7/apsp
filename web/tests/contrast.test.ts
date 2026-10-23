import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

// WCAG 2.1 AA asks for 4.5:1 on body text. Every colour in the interface comes
// from the design tokens in src/index.css, declared once for the light theme
// (`:root`) and once for the dark theme (`[data-theme="dark"]`), so the pairs
// below cover the text the components can paint. Browser-level coverage of the
// rendered pages lives in theme-contrast.mjs.

const css = readFileSync(fileURLToPath(new URL('../src/index.css', import.meta.url)), 'utf8')

function block(selector: string) {
  const start = css.indexOf(`${selector} {`)
  if (start < 0) throw new Error(`Missing ${selector} token block`)
  const body = css.slice(start, css.indexOf('}', start))
  return Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(([, name, value]) => [name, value.trim()]))
}

type Rgb = [number, number, number]
// oklch() -> linear sRGB, following the CSS Color 4 reference conversion.
function oklch(value: string): { rgb: Rgb; alpha: number } {
  const match = /^oklch\(([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+)%)?\)$/.exec(value)
  if (!match) throw new Error(`Unsupported colour ${value}`)
  const [l, c, h] = [Number(match[1]), Number(match[2]), Number(match[3]) * Math.PI / 180]
  const a = c * Math.cos(h), b = c * Math.sin(h)
  const l_ = (l + .3963377774 * a + .2158037573 * b) ** 3
  const m_ = (l - .1055613458 * a - .0638541728 * b) ** 3
  const s_ = (l - .0894841775 * a - 1.291485548 * b) ** 3
  const linear: Rgb = [
    4.0767416621 * l_ - 3.3077115913 * m_ + .2309699292 * s_,
    -1.2684380046 * l_ + 2.6097574011 * m_ - .3413193965 * s_,
    -.0041960863 * l_ - .7034186147 * m_ + 1.707614701 * s_,
  ]
  return { rgb: linear.map(channel => Math.min(1, Math.max(0, channel))) as Rgb, alpha: match[4] ? Number(match[4]) / 100 : 1 }
}
const encode = (linear: number) => linear <= .0031308 ? 12.92 * linear : 1.055 * linear ** (1 / 2.4) - .055
const decode = (value: number) => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4
// Translucent colours are composited in sRGB, as browsers paint them.
function over(front: { rgb: Rgb; alpha: number }, back: Rgb): Rgb {
  return front.rgb.map((channel, index) => decode(encode(channel) * front.alpha + encode(back[index]) * (1 - front.alpha))) as Rgb
}
const luminance = (rgb: Rgb) => .2126 * rgb[0] + .7152 * rgb[1] + .0722 * rgb[2]
function ratio(foreground: Rgb, background: Rgb) {
  const first = luminance(foreground), second = luminance(background)
  return (Math.max(first, second) + .05) / (Math.min(first, second) + .05)
}

const light = block(':root')
const dark = { ...light, ...block('[data-theme="dark"]') }

// [text token, background token, background tint over the surface (percent)]
const pairs: [string, string, number?][] = [
  ['--foreground', '--background'], ['--foreground', '--card'], ['--card-foreground', '--card'], ['--popover-foreground', '--popover'],
  ['--muted-foreground', '--background'], ['--muted-foreground', '--card'], ['--muted-foreground', '--muted'],
  ['--primary-foreground', '--primary'], ['--secondary-foreground', '--secondary'], ['--accent-foreground', '--accent'],
  ['--primary', '--card'], ['--primary', '--background'],
  // Destructive buttons paint a 10% tint behind their label: 20% on light hover and at rest in the dark theme.
  ['--destructive', '--card'], ['--destructive', '--destructive', 10], ['--destructive', '--destructive', 20],
  ['--success', '--card'], ['--success', '--success', 10],
  ['--warning', '--card'], ['--warning', '--warning', 10], ['--info', '--card'], ['--info', '--info', 10],
  ['--sidebar-foreground', '--sidebar'], ['--sidebar-accent-foreground', '--sidebar-accent'], ['--sidebar-primary-foreground', '--sidebar-primary'],
]

for (const [name, tokens] of [['light', light], ['dark', dark]] as const) {
  test(`${name} theme text tokens meet WCAG AA`, () => {
    const failures: string[] = []
    // Tints are measured over cards and over dialogs/popovers.
    for (const surfaceToken of ['--card', '--popover']) for (const [text, background, tint] of pairs) {
      if (!tint && surfaceToken !== '--card') continue
      const surface = oklch(tokens[surfaceToken]).rgb
      const back = tint ? over({ ...oklch(tokens[background]), alpha: tint / 100 }, surface) : oklch(tokens[background]).rgb
      const value = ratio(over(oklch(tokens[text]), back), back)
      if (value < 4.5) failures.push(`${text} on ${background}${tint ? ` ${tint}% over ${surfaceToken}` : ''}: ${value.toFixed(2)}`)
    }
    expect(failures).toEqual([])
  })
}

test('the dark theme redeclares every colour token', () => {
  const colourTokens = Object.keys(light).filter(token => /^oklch/.test(light[token]))
  const redeclared = Object.keys(block('[data-theme="dark"]'))
  expect(colourTokens.filter(token => !redeclared.includes(token))).toEqual([])
})

test('components use tokens instead of literal colours', () => {
  // Literal colours outside the token blocks would escape both themes.
  const outside = css.replace(/:root \{[^}]*\}/, '').replace(/\[data-theme="dark"\] \{[^}]*\}/, '')
  expect(outside.match(/#[0-9a-f]{3,8}\b|rgb\(|oklch\(/gi) ?? []).toEqual([])
})
