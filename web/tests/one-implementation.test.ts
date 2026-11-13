import { expect, test } from 'bun:test'
import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'

// ADR 0133 in the console: each mechanism has one implementation. These checks fail
// when a copy appears elsewhere in web/src; an exception names its file and reason.
const root = new URL('../src/', import.meta.url).pathname
const list = (dir: string): string[] => readdirSync(dir, { withFileTypes: true })
  .flatMap(entry => entry.isDirectory() ? list(join(dir, entry.name)) : /\.tsx?$/.test(entry.name) ? [join(dir, entry.name)] : [])
const files = list(root).map(path => ({ path: relative(root, path), text: readFileSync(path, 'utf8') }))

const rules: { mechanism: string; pattern: RegExp; home: string[]; exceptions?: Record<string, string> }[] = [
  {
    mechanism: 'times, numbers, sizes and money are formatted in format.ts',
    pattern: /\.toLocale(?:Date|Time)?String\(|Intl\.(?:DateTimeFormat|NumberFormat|RelativeTimeFormat)\b|['"]KiB['"],\s*['"]MiB['"]|[¥￥]\$\{/,
    home: ['format.ts'],
  },
  {
    mechanism: 'the signed-in administrator is read once, by lib/actor.ts',
    pattern: /\/api\/control-center\/me\b/,
    home: ['lib/actor.ts'],
  },
  {
    mechanism: 'notices, usage bars and status badges are the shared components',
    pattern: /\b(?:DisplayNotice|StatusPill)\b|<Track\b|display\/ui['"]/,
    home: [],
  },
  {
    mechanism: 'object, exact-key, integer, port and UUID checks are in lib/validate.ts',
    pattern: /typeof \w+ === 'object' && \w+ !== null|\w+ !== null && typeof \w+ === 'object'|!\w+ \|\| typeof \w+ !== 'object'|(?:<=|>) 65535\b|\[0-9a-f\]\{8\}-\[0-9a-f\]\{4\}-|const (?:object|record|integer|uuid|sourceUuid|validPort|exact) = \(|function (?:object|record|integer|uuid|validPort|exact)\(/,
    home: ['lib/validate.ts'],
    exceptions: { 'app/routes.ts': 'the proxy portal address matches its account UUID as part of the path' },
  },
]

for (const rule of rules) test(rule.mechanism, () => {
  const copies = files.filter(file => !rule.home.includes(file.path) && !(rule.exceptions && file.path in rule.exceptions) && rule.pattern.test(file.text)).map(file => file.path)
  expect(copies).toEqual([])
})

test('every exception still applies', () => {
  for (const rule of rules) for (const path of Object.keys(rule.exceptions ?? {})) {
    expect(rule.pattern.test(files.find(file => file.path === path)?.text ?? '')).toBe(true)
  }
})
