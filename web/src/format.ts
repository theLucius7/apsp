export function bytes(value: string | number | bigint | undefined | null): string {
  if (value === undefined || value === null) return '暂无数据'
  try {
    const amount = typeof value === 'number' ? BigInt(Math.max(0, Math.trunc(value))) : BigInt(value)
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB']
    let divisor = 1n, unit = 0
    while (amount >= divisor * 1024n && unit < units.length - 1) { divisor *= 1024n; unit++ }
    if (!unit) return `${amount} B`
    const scaled = amount * 10n / divisor
    return `${scaled / 10n}.${scaled % 10n} ${units[unit]}`
  } catch { return '暂无数据' }
}
export const totalBytes = (up: string, down: string) => (BigInt(up) + BigInt(down)).toString()
export const percent = (value?: number) => value === undefined ? '暂无数据' : `${value.toFixed(1)}%`
export const time = (value?: number | null) => value ? new Date(value * 1000).toLocaleString('zh-CN', { hour12: false }) : '尚未上报'
export function uptime(value?: number) {
  if (value === undefined) return '暂无数据'
  const days = Math.floor(value / 86400), hours = Math.floor(value % 86400 / 3600), minutes = Math.floor(value % 3600 / 60)
  return days ? `${days} 天 ${hours} 小时` : `${hours} 小时 ${minutes} 分钟`
}
export function navigate(path: string) { window.location.hash = path }

/** Coarse relative time for status lines, such as "12 分钟前"; the exact time belongs in a tooltip. */
export function ago(seconds?: number | null, now = Date.now() / 1000) {
  if (!seconds) return '时间未知'
  const elapsed = Math.max(0, now - seconds)
  if (elapsed < 60) return '刚刚'
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)} 分钟前`
  if (elapsed < 86400) return `${Math.floor(elapsed / 3600)} 小时前`
  return `${Math.floor(elapsed / 86400)} 天前`
}
