import { cn } from '@/lib/utils'

export function Region({ region }: { region?: string }) {
  const code = region?.trim().toUpperCase()
  if (!code) return null
  const flag = /^[A-Z]{2}$/.test(code) ? String.fromCodePoint(...[...code].map(letter => 0x1f1e6 + letter.charCodeAt(0) - 65)) : null
  let label = region
  if (flag) { try { label = new Intl.DisplayNames(['zh-CN'], { type: 'region' }).of(code) } catch { /* Keep the configured region when display names are unavailable. */ } }
  return <span className={cn('text-muted-foreground max-w-[70px] truncate', flag ? 'text-lg leading-none' : 'text-xs')} title={label} aria-label={label}>{flag ?? region}</span>
}

/** Operating-system logos are image assets; interface icons come from Lucide. */
export function OSIcon({ system }: { system?: string }) {
  const os = system?.toLowerCase() ?? ''
  const name = ['ubuntu', 'debian', 'alpine', 'arch', 'fedora', 'centos', 'rocky', 'gentoo', 'windows', 'macos', 'nix', 'alma'].find(value => os.includes(value))
    ?? (os.includes('darwin') || os.includes('mac os') ? 'macos' : 'unknown')
  return <img data-slot="os-icon" className="size-4 shrink-0 object-contain" src={`/display-icons/os-${name}.${name === 'alpine' ? 'webp' : 'svg'}`} width="16" height="16" alt={system || '系统未知'} title={system || '系统未知'} />
}
