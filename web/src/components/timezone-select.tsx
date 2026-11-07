import type { ComponentProps } from 'react'
import { NativeSelect } from '@/components/ui/native-select'
import { cn } from '@/lib/utils'

const commonZones: Record<string, string> = {
  'Asia/Shanghai': '北京 / 上海',
  'Asia/Tokyo': '东京',
  'Europe/London': '伦敦',
  'Europe/Berlin': '柏林',
  'America/New_York': '纽约',
  'America/Los_Angeles': '洛杉矶',
  UTC: '协调世界时',
}

type TimezoneSelectProps = Omit<ComponentProps<typeof NativeSelect>, 'children' | 'multiple' | 'value' | 'defaultValue'> & {
  defaultValue: string
}

export function TimezoneSelect({ defaultValue, className, ...props }: TimezoneSelectProps) {
  // Preserve an existing zone outside the short list without offering the full catalog.
  const savedOnly = !Object.hasOwn(commonZones, defaultValue)
  return <NativeSelect className={cn('w-full', className)} defaultValue={defaultValue} {...props}>
    {savedOnly && <option value={defaultValue}>{defaultValue}（当前设置）</option>}
    {Object.entries(commonZones).map(([zone, label]) => <option key={zone} value={zone}>{label} · {zone}</option>)}
  </NativeSelect>
}
