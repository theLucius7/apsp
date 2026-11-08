import { time } from '../../format'

/** The new expiry of a user's current package, as the set_validity operation states it. */
export type Until = { at: number } | 'never'

// Expiry is a moment the administrator picks on their own clock, so presets are calendar
// arithmetic in the browser's time zone; the panel stores the chosen moment unchanged and the
// preview shows it back before anything is applied.

export const expiryText = (seconds: number | null | undefined) => seconds === null || seconds === undefined ? '不限期' : time(seconds)

/** Adds calendar days or months; a month-end date stays at the end of a shorter month. */
export function addCalendar(seconds: number, amount: number, unit: 'day' | 'month'): number {
  const date = new Date(seconds * 1000)
  if (unit === 'day') date.setDate(date.getDate() + amount)
  else {
    const day = date.getDate()
    date.setDate(1)
    date.setMonth(date.getMonth() + amount)
    date.setDate(Math.min(day, new Date(date.getFullYear(), date.getMonth() + 1, 0).getDate()))
  }
  return Math.floor(date.getTime() / 1000)
}

const pad = (value: number) => String(value).padStart(2, '0')

/** The value of a `datetime-local` input for a moment, to the minute. */
export function localInput(seconds: number): string {
  const date = new Date(seconds * 1000)
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}

/** Reads a `datetime-local` value; a date-time without an offset is local time. */
export function fromLocalInput(value: string): number | null {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(value)) return null
  const milliseconds = new Date(value).getTime()
  return Number.isFinite(milliseconds) ? Math.floor(milliseconds / 1000) : null
}
