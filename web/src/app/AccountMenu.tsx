import { CheckIcon, ChevronsUpDownIcon, KeyRoundIcon, LogOutIcon, MonitorIcon, MoonIcon, SunIcon } from 'lucide-react'
import { usePreference } from '../control-center/preferences'
import { localeChosenEvent } from '../control-center/Appearance'
import { useI18n } from '../i18n'
import type { Locale } from '../i18n'
import { cn } from '@/lib/utils'
import type { ShellActor } from '@/lib/shell-actor'
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
  return <DropdownMenu modal={false}>
    <DropdownMenuTrigger asChild>
      <button type="button" data-slot="account-menu" aria-label={`${name} · ${t('账号选项')}`}
        className={cn('hover:bg-sidebar-accent focus-visible:ring-sidebar-ring/50 aria-expanded:bg-sidebar-accent flex w-full min-w-0 items-center gap-2 rounded-md p-1 text-left outline-none focus-visible:ring-3', collapsed && 'justify-center')}>
        <span className="bg-primary/10 text-primary grid size-6 shrink-0 place-items-center rounded-full text-[11px] font-semibold" aria-hidden="true">{initial}</span>
        {!collapsed && <>
          <span className="min-w-0 flex-1 truncate text-[13px] font-medium">{name}</span>
          <small className="text-muted-foreground shrink-0 text-xs">{role}</small>
          <ChevronsUpDownIcon className="text-muted-foreground size-3.5 shrink-0" aria-hidden="true" />
        </>}
      </button>
    </DropdownMenuTrigger>
    <DropdownMenuContent side={collapsed ? 'right' : 'top'} align="start" className="w-52">
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
