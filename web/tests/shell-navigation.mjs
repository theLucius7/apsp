// TEST_ONLY: how a page is reached in the console. Pages about the same thing are tabs
// of one sidebar entry, and experimental workspaces are hidden until the settings show them.
const entries = { 统计报表: '概览', 服务器列表: '服务器', 服务器看板: '概览', 告警事件: '告警', 告警设置: '告警', 网络检测: '线路检测', 延迟检测: '线路检测', 面板设置: '设置', 插件中心: '设置', 备份与恢复: '设置', 管理与安全: '设置', 账号安全: '设置' }
const addresses = { 统计报表: '/statistics', 服务器看板: '/dashboard', 告警设置: '/monitoring/settings', 延迟检测: '/latency', 插件中心: '/plugins/catalog', 备份与恢复: '/system/backup', 管理与安全: '/system/control-center', 账号安全: '/system/administrator', 自动化与维护: '/operations', 网络配置: '/network-configuration' }

export async function openPage(page, name, options = {}) {
  // A tab or a hidden workspace is opened by address, so the page in front of it is not read on the way.
  if (addresses[name]) return page.evaluate(path => { window.location.hash = `#${path}` }, addresses[name])
  const navigation = options.navigation ?? page.getByRole('navigation', { name: '主导航' })
  await navigation.getByRole('link', { name: entries[name] ?? name, exact: true }).click()
}

export async function signOut(page) {
  const menu = page.getByRole('button', { name: '菜单', exact: true })
  if (await menu.isVisible()) { await menu.click(); await page.locator('#admin-navigation [data-slot=logout-button]').click(); return }
  await page.locator('[data-slot=account-menu]').first().click()
  await page.getByRole('menuitem', { name: '退出登录' }).click()
}

export async function chooseLanguage(page, label) {
  await page.locator('[data-slot=account-menu]').first().click()
  await page.getByRole('menuitem').filter({ hasText: label }).click()
}
