import type { QualityField } from '../types'

const unavailable = new Set(['null', 'undefined', 'unknown', 'n/a', 'none', 'nan', '-', '未知'])
const rawNumber = /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i
type FieldKind = NonNullable<QualityField['kind']>
const kinds: FieldKind[] = ['text', 'country_code', 'boolean', 'score', 'asn', 'latitude', 'longitude']
const legacyKinds: Record<string, Record<string, FieldKind>> = {
  'ipregistry-v1': { ASN: 'asn', 网络组织: 'text', 连接类型: 'text', 国家代码: 'country_code', 代理: 'boolean', Tor: 'boolean', VPN: 'boolean', 滥用: 'boolean', 攻击来源: 'boolean', 云服务商: 'boolean' },
  'dbip-v2': { 国家代码: 'country_code', 国家或地区: 'text', ASN: 'asn', 网络组织: 'text', ISP: 'text', 用途类型: 'text', 代理: 'boolean', 爬虫: 'boolean', 纬度: 'latitude', 经度: 'longitude' },
  maxmind: { ASN: 'asn', 网络组织: 'text', 国家或地区: 'text', 国家代码: 'country_code', 城市: 'text', 纬度: 'latitude', 经度: 'longitude', 时区: 'text' },
  ipapi: { 'ASN 类型': 'text', 组织类型: 'text', '滥用评分（上游原值）': 'score', 国家代码: 'country_code', 代理: 'boolean', Tor: 'boolean', VPN: 'boolean', 数据中心: 'boolean', 滥用: 'boolean', 爬虫: 'boolean' },
  scamalytics: { '风险评分（上游原值）': 'score', VPN: 'boolean', 数据中心: 'boolean', 外部黑名单: 'boolean', 'FireHOL 代理': 'boolean', 'X4B Tor': 'boolean', 国家代码: 'country_code' },
  abuseipdb: { 用途类型: 'text', '滥用置信度（上游原值）': 'score' },
  'abuseipdb-v2': { 用途类型: 'text', 国家代码: 'country_code', ISP: 'text', Tor: 'boolean', '滥用置信度（0–100 原值）': 'score' },
  ip2location: { '欺诈评分（上游原值）': 'score', 国家代码: 'country_code', 用途类型: 'text', 'ASN 用途': 'text', 代理: 'boolean', 公共代理: 'boolean', 网页代理: 'boolean', Tor: 'boolean', VPN: 'boolean', 数据中心: 'boolean', 垃圾邮件: 'boolean', 爬虫: 'boolean', 扫描器: 'boolean', 僵尸网络: 'boolean' },
  ipdata: { 国家代码: 'country_code', 代理: 'boolean', Tor: 'boolean', 数据中心: 'boolean', 威胁: 'boolean', 已知滥用: 'boolean', 已知攻击者: 'boolean' },
  ipqualityscore: { '欺诈评分（上游原值）': 'score', 国家代码: 'country_code', 代理: 'boolean', Tor: 'boolean', VPN: 'boolean', 近期滥用: 'boolean', 机器人: 'boolean' },
}

const meaningfulText = (value: string) => Boolean(value.trim()) && !unavailable.has(value.trim().toLowerCase())

export function qualityValue(field: QualityField, database?: string): string | undefined {
  const value = field.value
  // Older payloads omit kind; only registered labels have a known semantic type.
  const definitions = database && Object.hasOwn(legacyKinds, database) ? legacyKinds[database] : undefined
  const kind = field.kind ?? (definitions && Object.hasOwn(definitions, field.label) ? definitions[field.label] : undefined)
  if (kind != null && !kinds.includes(kind)) return undefined
  if (value == null || typeof value === 'object') return undefined
  if (typeof value === 'string' && !meaningfulText(value)) return undefined
  if (database === 'abuseipdb-v2' && kind === 'score' && (typeof value !== 'number' || !Number.isInteger(value) || value < 0 || value > 100)) return undefined
  if (kind === 'boolean' && typeof value !== 'boolean') return undefined
  if (kind === 'text' && typeof value !== 'string') return undefined
  if (kind === 'country_code' && (typeof value !== 'string' || !/^[a-z]{2}$/i.test(value))) return undefined
  if (['score', 'asn', 'latitude', 'longitude'].includes(kind ?? '')) {
    let numericText = value
    if (typeof value === 'string' && kind === 'score' && !rawNumber.test(value.trim())) {
      const text = value.trim(), separator = text.indexOf(' ')
      const rating = text.slice(separator + 1).trim()
      if (separator < 0 || !rating.startsWith('(') || !rating.endsWith(')') || !meaningfulText(rating.slice(1, -1))) return undefined
      numericText = text.slice(0, separator)
    }
    if (typeof numericText === 'boolean' || typeof numericText === 'string' && !rawNumber.test(numericText.trim())) return undefined
    const numeric = Number(numericText)
    if (!Number.isFinite(numeric)) return undefined
    if (kind === 'score' && numeric < 0 || kind === 'asn' && (numeric <= 0 || numeric > 4294967295 || !Number.isInteger(numeric))) return undefined
    if (kind === 'latitude' && Math.abs(numeric) > 90 || kind === 'longitude' && Math.abs(numeric) > 180) return undefined
  }
  if (typeof value === 'boolean') return value ? '是' : '否'
  if (typeof value === 'number') return Number.isFinite(value) ? String(value) : undefined
  return typeof value === 'string' ? value : undefined
}
