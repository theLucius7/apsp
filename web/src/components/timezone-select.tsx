import type { ComponentProps } from 'react'
import { NativeSelect } from '@/components/ui/native-select'
import { cn } from '@/lib/utils'

const commonZones: Record<string, string> = {
  'Asia/Shanghai': '北京 / 上海',
  'Asia/Hong_Kong': '香港',
  'Asia/Taipei': '台北',
  'Asia/Tokyo': '东京',
  'Asia/Singapore': '新加坡',
  'Europe/London': '伦敦',
  'Europe/Berlin': '柏林',
  'America/New_York': '纽约',
  'America/Los_Angeles': '洛杉矶',
  UTC: '协调世界时',
}

const otherZones = typeof Intl.supportedValuesOf === 'function'
  ? Intl.supportedValuesOf('timeZone').filter(zone => !Object.hasOwn(commonZones, zone))
  : []

type TimezoneSelectProps = Omit<ComponentProps<typeof NativeSelect>, 'children' | 'multiple' | 'value' | 'defaultValue'> & {
  defaultValue: string
}

export function TimezoneSelect({ defaultValue, className, ...props }: TimezoneSelectProps) {
  // Keep the exact saved identifier, including IANA aliases absent from Intl's list.
  const savedOnly = !Object.hasOwn(commonZones, defaultValue) && !otherZones.includes(defaultValue)
  return <NativeSelect className={cn('w-full', className)} defaultValue={defaultValue} {...props}>
    {savedOnly && <option value={defaultValue}>{defaultValue}（当前设置）</option>}
    <optgroup label="常用时区">
      {Object.entries(commonZones).map(([zone, label]) => <option key={zone} value={zone}>{label} · {zone}</option>)}
    </optgroup>
    {otherZones.length > 0 && <optgroup label="其他时区">
      {otherZones.map(zone => <option key={zone} value={zone}>{zone}</option>)}
    </optgroup>}
  </NativeSelect>
}
