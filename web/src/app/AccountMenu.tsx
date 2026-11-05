import { CheckIcon, ChevronsUpDownIcon, KeyRoundIcon, LogOutIcon, MonitorIcon, MoonIcon, SunIcon } from 'lucide-react'
import { usePreference } from '../control-center/preferences'
import { localeChosenEvent } from '../control-center/Appearance'
import { useI18n } from '../i18n'
import type { Locale } from '../i18n'
import { cn } from '@/lib/utils'
import type { ShellActor } from '@/lib/shell-actor'
import { useConnectionStore } from '@/stores/connection'
import type { Density, ThemePreference } from '@/stores/appearance'
import { useAppearanceStore } from '@/stores/appearance'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'

type Saved = { theme: ThemePreference; density: Density; locale?: Locale }
const themes: { value: ThemePreference; label: string; icon: typeof SunIcon }[] = [
  { value: 'system', label: '跟随系统', icon: MonitorIcon },
  { value: 'light', label: '浅色', icon: SunIcon },
  { value: 'dark', label: '深色', icon: MoonIcon },
]
const densities: { value: Density; label: string }[] = [{ value: 'comfortable', label: '舒适' }, { value: 'compact', label: '紧凑' }]
const locales: { value: Locale; label: string }[] = [{ value: 'zh-CN', label: '简体中文' }, { value: 'en-US', label: 'English' }]

/** Who is signed in, the choices that follow them everywhere, and the way out. */
export default function AccountMenu({ actor, busy, onLogout, collapsed }: { actor?: ShellActor; busy: boolean; onLogout: () => void; collapsed: boolean }) {
  const { t, locale, setLocale } = useI18n()
  const preference = usePreference<Saved>('appearance')
  const theme = useAppearanceStore(state => state.theme)
  const density = useAppearanceStore(state => state.density)
  const setAppearance = useAppearanceStore(state => state.setAppearance)
  const role = actor?.role === 'viewer' ? t('只读') : actor?.role === 'operator' ? t('运维') : actor?.role === 'owner' ? t('所有者') : t('管理员')
  const name = actor?.display_name ?? t('管理员')
  const initial = [...name][0]?.toLocaleUpperCase() ?? '管'
  // A failed save keeps the choice for this session; the stored value applies next time.
  const save = (next: Partial<Saved>) => { if (preference.ready) void preference.save({ theme: preference.value?.theme ?? theme, density: preference.value?.density ?? density, ...(preference.value?.locale ? { locale: preference.value.locale } : {}), ...next }).catch(() => {}) }
  // The connection to the panel reads on the avatar: one place for "who" and "connected".
  const status = useConnectionStore(state => state.status)
  const connection = t(status === 'online' ? '已连接面板' : status === 'offline' ? '网络已断开' : '正在重新连接')
  const secondary = [name === role ? '' : role, connection].filter(Boolean).join(' · ')
  // The primary colour carries the letter (its pair passes AA in both themes); the gradient only leans towards the brand cyan.
  const avatar = <span className="bg-primary from-primary to-[color-mix(in_oklch,var(--primary),var(--brand-to)_38%)] text-primary-foreground ring-primary-foreground/15 relative grid size-9 shrink-0 place-items-center rounded-[11px] bg-gradient-to-br text-[13px] font-semibold shadow-soft ring-1 ring-inset" aria-hidden="true">
    {initial}
    <span data-slot="connection-dot" data-status={status} className={cn('ring-sidebar absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2', status === 'online' ? 'bg-success' : status === 'offline' ? 'bg-destructive' : 'bg-warning animate-pulse')} />
  </span>
  return <DropdownMenu modal={false}>
    <span data-slot="connection-status" role="status" className="sr-only">{connection}</span>
    <DropdownMenuTrigger asChild>
      <button type="button" data-slot="account-menu" aria-label={`${name} · ${t('账号选项')}`} title={collapsed ? `${name} · ${secondary}` : undefined}
        className={cn('group/account flex w-full min-w-0 items-center gap-2.5 rounded-xl text-left outline-none transition-[background-color,border-color,box-shadow] duration-150 focus-visible:shadow-focus',
          collapsed ? 'justify-center p-1 hover:bg-sidebar-accent/60 aria-expanded:bg-sidebar-accent/60'
            : 'border-sidebar-border/80 bg-sidebar-accent/55 shadow-soft hover:border-[color-mix(in_oklch,var(--primary)_28%,var(--sidebar-border))] hover:bg-sidebar-accent aria-expanded:bg-sidebar-accent border p-2')}>
        {avatar}
        {!collapsed && <>
          <span className="grid min-w-0 flex-1 leading-tight">
            <strong className="truncate text-[13px] font-semibold">{name}</strong>
            <small className="text-muted-foreground mt-0.5 truncate text-[11.5px]">{secondary}</small>
          </span>
          <ChevronsUpDownIcon className="text-muted-foreground group-hover/account:text-foreground size-4 shrink-0 transition-colors" aria-hidden="true" />
        </>}
      </button>
    </DropdownMenuTrigger>
    <DropdownMenuContent side={collapsed ? 'right' : 'top'} align="start" className="w-60">
      <div className="flex items-center gap-2.5 px-2 pt-1.5 pb-2">{avatar}<span className="grid min-w-0 leading-tight"><strong className="truncate text-[13px] font-semibold">{name}</strong><small className="text-muted-foreground mt-0.5 truncate text-[11.5px]">{role}</small></span></div>
      <DropdownMenuSeparator />
      <DropdownMenuLabel className="text-muted-foreground text-xs font-medium">{t('主题')}</DropdownMenuLabel>
      {themes.map(item => {
        const Icon = item.icon
        return <DropdownMenuItem key={item.value} onSelect={() => { setAppearance({ theme: item.value }); save({ theme: item.value }) }}>
          <Icon aria-hidden="true" /><span className="flex-1">{t(item.label)}</span>{theme === item.value && <CheckIcon className="text-primary" aria-label={t('当前项')} />}
        </DropdownMenuItem>
      })}
      <DropdownMenuSeparator />
      <DropdownMenuLabel className="text-muted-foreground text-xs font-medium">{t('密度')}</DropdownMenuLabel>
      {densities.map(item => <DropdownMenuItem key={item.value} onSelect={() => { setAppearance({ density: item.value }); save({ density: item.value }) }}>
        <span className="flex-1">{t(item.label)}</span>{density === item.value && <CheckIcon className="text-primary" aria-label={t('当前项')} />}
      </DropdownMenuItem>)}
      <DropdownMenuSeparator />
      <DropdownMenuLabel className="text-muted-foreground text-xs font-medium">{t('界面语言')}</DropdownMenuLabel>
      {locales.map(item => <DropdownMenuItem key={item.value} onSelect={() => { setLocale(item.value); window.dispatchEvent(new CustomEvent(localeChosenEvent, { detail: item.value })) }}>
        <span className="flex-1">{item.label}</span>{locale === item.value && <CheckIcon className="text-primary" aria-label={t('当前项')} />}
      </DropdownMenuItem>)}
      <DropdownMenuSeparator />
      <DropdownMenuItem onSelect={() => { window.location.hash = '#/system/administrator' }}><KeyRoundIcon aria-hidden="true" />{t('账号安全')}</DropdownMenuItem>
      <DropdownMenuItem data-slot="logout-button" disabled={busy} onSelect={onLogout}><LogOutIcon aria-hidden="true" />{t('退出登录')}</DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
}
