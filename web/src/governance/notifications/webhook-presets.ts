export type Preset = 'custom' | 'bark' | 'discord' | 'slack' | 'wecom' | 'dingtalk' | 'feishu' | 'ntfy' | 'gotify'
const text = '{{title}}\n{{server}}\n{{message}}'
const body = (value: unknown) => JSON.stringify(value, null, 2)
export const webhookPresets: { id: Preset; name: string; url: string; headers: string; body: string; hint: string }[] = [
  { id: 'custom', name: '通用 JSON', url: '', headers: '', body: body({ event: '{{event}}', event_id: '{{event_id}}', category: '{{category}}', node: '{{server}}', title: '{{title}}', message: '{{message}}', site: '{{site}}', time: '{{time}}' }), hint: '适用于自建接收端；收到 HTTP 2xx 视为接收成功。' },
  { id: 'bark', name: 'Bark', url: 'https://api.day.app/push', headers: '', body: body({ device_key: '填写设备密钥', title: '{{title}}', subtitle: '{{server}}', body: '{{message}}', group: 'apsp' }), hint: '填写设备密钥；也可使用自己的 Bark 服务地址。' },
  { id: 'discord', name: 'Discord', url: '', headers: '', body: body({ content: text, allowed_mentions: { parse: [] } }), hint: '粘贴频道的 Webhook 地址；默认禁止自动提及成员。' },
  { id: 'slack', name: 'Slack', url: '', headers: '', body: body({ text }), hint: '粘贴工作区的传入 Webhook 地址。' },
  { id: 'wecom', name: '企业微信', url: 'https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=填写密钥', headers: '', body: body({ msgtype: 'text', text: { content: text } }), hint: '使用群机器人地址，需将地址中的密钥替换为实际值。' },
  { id: 'dingtalk', name: '钉钉', url: 'https://oapi.dingtalk.com/robot/send?access_token=填写令牌', headers: '', body: body({ msgtype: 'text', text: { content: `apsp\n${text}` } }), hint: '可将安全关键词设置为“apsp”；当前不支持动态签名方式。' },
  { id: 'feishu', name: '飞书', url: '', headers: '', body: body({ msg_type: 'text', content: { text: `apsp\n${text}` } }), hint: '使用自定义机器人地址，可配置“apsp”关键词；当前不支持动态签名方式。' },
  { id: 'ntfy', name: 'ntfy', url: 'https://ntfy.sh', headers: '', body: body({ topic: '填写主题', title: '{{title}}', message: '{{server}}\n{{message}}' }), hint: '地址填写 ntfy 服务根地址，主题填在 JSON 的 topic 中；私有主题的认证可放在请求头中。' },
  { id: 'gotify', name: 'Gotify', url: '', headers: 'X-Gotify-Key: 填写应用令牌', body: body({ title: '{{title}}', message: '{{server}}\n{{message}}', priority: 5 }), hint: '填写 Gotify 服务的 /message 地址及应用令牌。' },
]
export const webhookExamples: Record<string, string> = { title: '资源超限告警', server: '示例服务器', node: '示例服务器', message: '处理器使用率达到设定阈值。', time: '示例时间（UTC）', event: 'resource', event_id: '123', category: 'resource', site: 'apsp' }
export function previewWebhook(template: string): string {
  try {
    const replace = (value: unknown): unknown => {
      if (typeof value === 'string') return value.replace(/\{\{([^{}]+)\}\}/g, (token, key: string) => webhookExamples[key] ?? token)
      if (Array.isArray(value)) return value.map(replace)
      if (value !== null && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, value]) => [key, replace(value)]))
      return value
    }
    return body(replace(JSON.parse(template)))
  } catch { return '请输入有效 JSON；占位符需位于字符串内。' }
}
