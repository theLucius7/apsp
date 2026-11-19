import { nonnegativeNumber as number } from './validate'

export type Point = { at: number; value: number | null; range?: { from: number; to: number; count: number; min: number; max: number; samples: number; partial: boolean; live?: boolean; bucketFrom?: number; bucketTo?: number } }

// Keep missing samples and long gaps separate. Limit SVG complexity without hiding peaks.
export function segments(points: Point[], from: number, to: number, gap: number, buckets = 180): Point[][] {
  const ordered = points.filter(point => Number.isFinite(point.at) && point.at >= from && point.at <= to)
    .sort((left, right) => left.at - right.at)
  const result: Point[][] = []
  let current: Point[] = []
  for (const point of ordered) {
    if (point.value === null || number(point.value) === null || (current.length && point.at - current[current.length - 1].at > gap)) {
      if (current.length) result.push(current)
      current = []
    }
    if (number(point.value) !== null) current.push(point)
  }
  if (current.length) result.push(current)
  const width = Math.max(1, (to - from) / buckets)
  return result.map(part => {
    const groups = new Map<number, Point[]>()
    for (const point of part) {
      const key = Math.floor((point.at - from) / width)
      const group = groups.get(key) ?? []
      group.push(point)
      groups.set(key, group)
    }
    return [...groups.values()].flatMap(group => {
      const low = group.reduce((best, point) => (point.range?.min ?? point.value!) < (best.range?.min ?? best.value!) ? point : best)
      const high = group.reduce((best, point) => (point.range?.max ?? point.value!) > (best.range?.max ?? best.value!) ? point : best)
      return [...new Set([group[0], low, high, group[group.length - 1]])].sort((a, b) => a.at - b.at)
    })
  })
}
