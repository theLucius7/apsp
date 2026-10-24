import { createElement, useMemo } from 'react'
import type { ReactNode } from 'react'

const allowed = new Set('p br strong em b i u s blockquote pre code ul ol li hr a img table thead tbody tr th td div span details summary h1 h2 h3 h4 h5 h6'.split(' '))
const hidden = new Set(['script', 'style', 'iframe', 'object', 'embed', 'svg', 'math', 'form', 'input', 'button', 'link', 'meta'])
function safeUrl(raw: string | null, image = false): string | undefined {
  if (!raw) return
  try { const url = new URL(raw, window.location.href); if ((image ? ['https:', 'http:'] : ['https:', 'http:', 'mailto:', 'clash:', 'shadowrocket:']).includes(url.protocol) && !url.username && !url.password) return url.href } catch { /* Invalid links remain text. */ }
}
export default function GuideBody({ body }: { body: string }) {
  const children = useMemo(() => {
    if (!/<[a-z][\s\S]*>/i.test(body)) return body
    const document = new DOMParser().parseFromString(body, 'text/html')
    let count = 0
    function render(node: Node, depth: number): ReactNode {
      if (++count > 20000 || depth > 40) return null
      if (node.nodeType === Node.TEXT_NODE) return node.textContent
      if (node.nodeType !== Node.ELEMENT_NODE) return null
      const element = node as Element, tag = element.tagName.toLowerCase(), key = count
      if (hidden.has(tag)) return null
      const nested = Array.from(element.childNodes).map(n => render(n, depth + 1))
      if (!allowed.has(tag)) return nested
      if (tag === 'img') { const src = safeUrl(element.getAttribute('src'), true); return src ? <img key={key} src={src} alt={element.getAttribute('alt') ?? ''} loading="lazy" referrerPolicy="no-referrer" className="my-3 max-w-full rounded-lg" /> : null }
      if (tag === 'br' || tag === 'hr') return createElement(tag, { key })
      return createElement(tag, { key, ...(tag === 'a' ? { href: safeUrl(element.getAttribute('href')), target: '_blank', rel: 'noreferrer noopener' } : {}) }, nested)
    }
    return Array.from(document.body.childNodes).map(n => render(n, 0))
  }, [body])
  return <div data-slot="guide-body" className="whitespace-pre-wrap wrap-anywhere leading-relaxed [&_h1]:text-lg [&_h2]:text-base [&_h3]:font-semibold [&_p]:my-2 [&_ul]:list-inside [&_ul]:list-disc [&_ol]:list-inside [&_ol]:list-decimal [&_a]:text-primary [&_a]:underline [&_pre]:overflow-auto [&_pre]:rounded-lg [&_pre]:bg-muted/50 [&_pre]:p-3 [&_table]:block [&_table]:overflow-auto [&_td]:border [&_td]:p-2 [&_th]:border [&_th]:p-2">{children}</div>
}
