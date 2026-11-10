import { expect, test } from 'bun:test'
import { addCalendar, fromLocalInput, localInput } from '../src/access/validity'

// Built from local calendar fields, so the checks hold in whatever time zone the test runs.
const at = (year: number, month: number, day: number, hour = 0, minute = 0) => Math.floor(new Date(year, month - 1, day, hour, minute).getTime() / 1000)

test('months keep the day of month and stay at month end in shorter months', () => {
  expect(addCalendar(at(2027, 1, 15, 9, 30), 1, 'month')).toBe(at(2027, 2, 15, 9, 30))
  expect(addCalendar(at(2027, 1, 31, 23, 59), 1, 'month')).toBe(at(2027, 2, 28, 23, 59))
  expect(addCalendar(at(2028, 1, 31, 23, 59), 1, 'month')).toBe(at(2028, 2, 29, 23, 59))
  expect(addCalendar(at(2027, 1, 31, 23, 59), 3, 'month')).toBe(at(2027, 4, 30, 23, 59))
  expect(addCalendar(at(2026, 11, 30), 12, 'month')).toBe(at(2027, 11, 30))
  expect(addCalendar(at(2026, 12, 31), 2, 'month')).toBe(at(2027, 2, 28))
})

test('days follow the local calendar', () => {
  expect(addCalendar(at(2026, 12, 25, 8), 10, 'day')).toBe(at(2027, 1, 4, 8))
})

test('the picker value round-trips to the minute and rejects incomplete input', () => {
  const moment = at(2027, 3, 8, 23, 59)
  expect(localInput(moment)).toBe('2027-03-08T23:59')
  expect(fromLocalInput('2027-03-08T23:59')).toBe(moment)
  expect(localInput(moment + 45)).toBe('2027-03-08T23:59')
  for (const value of ['', '2027-03-08', '2027-03-08T23', 'not a date']) expect(fromLocalInput(value)).toBeNull()
})
