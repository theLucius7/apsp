// Page addresses name what a page is for, not which plugin implements it (ADR 0120).
// Addresses published before are rewritten to their current form when they are
// opened, so bookmarks, sign-in addresses and invitation links sent earlier keep
// working; nothing new is generated in the old form.

/** Old prefix → current prefix, longest first so a more specific one wins. */
const renamed: readonly (readonly [string, string])[] = [
  ['/plugins/sing-box/commerce', '/users'],
  ['/plugins/sing-box/account', '/account'],
  ['/plugins/sing-box/member', '/member'],
  ['/plugins/sing-box/join', '/join'],
  ['/plugins/sing-box/users', '/users'],
  ['/plugins/sing-box/groups', '/groups'],
  ['/plugins/sing-box/nodes', '/nodes'],
  ['/plugins/sing-box/chains', '/nodes?kind=chains'],
  ['/plugins/sing-box', '/home'],
  ['/plugins/expenses', '/expenses'],
  ['/plugins/ddns', '/dns'],
  ['/plugins/alicloud', '/alicloud'],
  ['/plugins/catalog', '/settings/plugins'],
  ['/system/plugins', '/settings/plugins/servers'],
  ['/system/settings', '/settings'],
  ['/system/backup', '/settings/backup'],
  ['/system/control-center', '/settings/admin'],
  ['/system/administrator', '/settings/account'],
  ['/system/notifications', '/alerts'],
  ['/monitoring/settings', '/alerts/settings'],
  ['/artifacts', '/settings/plugins'],
]

/** The current address of a page; an address already current is returned unchanged. */
export function canonicalPath(path: string): string {
  for (const [old, current] of renamed) {
    if (path !== old && !path.startsWith(`${old}/`) && !path.startsWith(`${old}?`)) continue
    const rest = path.slice(old.length)
    if (!current.includes('?')) return current + rest
    // An address that already carries a query takes further parameters, never a sub-page.
    return rest.startsWith('/') ? path : current + rest.replace(/^\?/, '&')
  }
  return path
}
