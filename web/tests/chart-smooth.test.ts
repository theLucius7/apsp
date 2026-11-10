import { expect, test } from 'bun:test'
import { smoothPath } from '../src/observability/display/Chart'

// Reads the cubic segments back from the path string.
function segments(path: string) {
  const numbers = path.replace(/[MC]/g, ' ').trim().split(/[\s,]+/).map(Number)
  const start: [number, number] = [numbers[0], numbers[1]], result: [number, number][][] = []
  let previous = start
  for (let index = 2; index + 5 < numbers.length; index += 6) {
    const segment: [number, number][] = [previous, [numbers[index], numbers[index + 1]], [numbers[index + 2], numbers[index + 3]], [numbers[index + 4], numbers[index + 5]]]
    result.push(segment)
    previous = segment[3]
  }
  return result
}
const bezier = (points: [number, number][], t: number) => {
  const u = 1 - t
  return [0, 1].map(axis => u ** 3 * points[0][axis] + 3 * u * u * t * points[1][axis] + 3 * u * t * t * points[2][axis] + t ** 3 * points[3][axis])
}

test('the smoothed line passes through every sample', () => {
  const samples: [number, number][] = [[0, 50], [10, 10], [20, 90], [30, 90], [40, 30]]
  const parts = segments(smoothPath(samples))
  expect(parts.length).toBe(samples.length - 1)
  parts.forEach((part, index) => {
    expect(part[0]).toEqual(samples[index])
    expect(part[3]).toEqual(samples[index + 1])
  })
})

test('the smoothed line never overshoots between two samples', () => {
  // A spike and a plateau are where a naive spline would invent a peak or a dip.
  const samples: [number, number][] = [[0, 100], [10, 100], [20, 5], [30, 100], [40, 100], [50, 60], [60, 0]]
  for (const [index, part] of segments(smoothPath(samples)).entries()) {
    const low = Math.min(samples[index][1], samples[index + 1][1]) - 1e-6, high = Math.max(samples[index][1], samples[index + 1][1]) + 1e-6
    for (let step = 0; step <= 20; step++) {
      const [, y] = bezier(part, step / 20)
      expect(y).toBeGreaterThanOrEqual(low)
      expect(y).toBeLessThanOrEqual(high)
    }
  }
})

test('one or two samples stay straight', () => {
  expect(smoothPath([[1, 2]])).toBe('M1.00,2.00')
  expect(smoothPath([[1, 2], [3, 4]])).toBe('M1.00,2.00 L3.00,4.00')
})
