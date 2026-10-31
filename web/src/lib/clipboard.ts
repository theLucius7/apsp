/** Copies text, falling back to the older command where the clipboard API is unavailable. */
export async function copyText(text: string) {
  if (navigator.clipboard?.writeText) {
    try { await navigator.clipboard.writeText(text); return } catch { /* Try the older command below. */ }
  }
  const input = document.createElement('textarea')
  input.value = text; input.style.position = 'fixed'; input.style.opacity = '0'; document.body.append(input)
  try { input.select(); if (!document.execCommand('copy')) throw new Error('clipboard unavailable') }
  finally { input.remove() }
}
