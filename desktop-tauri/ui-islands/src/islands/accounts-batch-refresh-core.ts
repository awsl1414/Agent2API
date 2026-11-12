export const BATCH_ACTION_VALUES = [
  'enable', 'disable', 'refresh', 'proxy', 'concurrency', 'usageQuery', 'limiters', 'remove',
] as const

export type BatchAction = typeof BATCH_ACTION_VALUES[number]

export type BatchRefreshAccount = {
  id: string
  provider?: string
  desktop?: boolean
  canRefresh?: boolean
  hasRefreshToken?: boolean
  needsRelogin?: boolean
  [key: string]: unknown
}

export type RefreshBridgeResult = {
  refreshedId?: string
  list?: { accounts?: BatchRefreshAccount[] }
} | null | undefined

export type RefreshProgress = { completed: number; total: number; name: string }

export type RefreshMessages = {
  unavailable: string
  notFound: string
  unconfirmed: string
}

export type RefreshSkipTexts = {
  desktop: string
  relogin: string
  unsupported: string
}

export type BatchRefreshResult = {
  ok: Array<{ id: string; name: string }>
  failed: Array<{ id: string; error: string }>
  skipped: Array<{ id: string; name: string; reason: string }>
}

export type RefreshDependencies = {
  findAccount: (id: string) => BatchRefreshAccount | null
  refreshAccountToken?: (id: string) => Promise<RefreshBridgeResult>
  nameOf?: (account: BatchRefreshAccount | null) => string
  providerOf?: (account: BatchRefreshAccount | null | undefined) => string
  skipReason?: (account: BatchRefreshAccount) => string
  onProgress?: (progress: RefreshProgress) => void
  messages: RefreshMessages
  errorMessage?: (error: unknown) => string
}

export function supportsTokenRefreshCore(account: BatchRefreshAccount): boolean {
  return account.desktop !== true && (account.canRefresh === true || account.hasRefreshToken === true)
}

export function tokenRefreshSkipReasonCore(account: BatchRefreshAccount, texts: RefreshSkipTexts): string {
  if (account.desktop === true) return texts.desktop
  if (account.needsRelogin === true) return texts.relogin
  if (!supportsTokenRefreshCore(account)) return texts.unsupported
  return ''
}

export async function runBatchTokenRefresh(
  ids: string[],
  deps: RefreshDependencies,
): Promise<BatchRefreshResult> {
  const refresh = deps.refreshAccountToken
  if (typeof refresh !== 'function') throw new Error(deps.messages.unavailable)

  const selected = [...new Set(ids)]
  const ok: BatchRefreshResult['ok'] = []
  const failed: BatchRefreshResult['failed'] = []
  const skipped: BatchRefreshResult['skipped'] = []
  const nameOf = deps.nameOf ?? ((item: BatchRefreshAccount | null) => String(item?.name || item?.id || ''))
  const providerOf = deps.providerOf ?? ((item: BatchRefreshAccount | null | undefined) => String(item?.provider || ''))
  const skipReason = deps.skipReason ?? (() => '')
  const messageOfError = deps.errorMessage ?? ((error: unknown) => error instanceof Error ? error.message : String(error))

  // 桌面桥的单次请求限时 60 秒，整组刷新必须拆成逐账号请求。
  for (const [index, id] of selected.entries()) {
    const account = deps.findAccount(id)
    const name = nameOf(account) || id
    deps.onProgress?.({ completed: index, total: selected.length, name })
    try {
      if (!account) throw new Error(deps.messages.notFound)
      const reason = skipReason(account)
      if (reason) {
        skipped.push({ id, name, reason })
      } else {
        const data = await refresh(id)
        const refreshed = data?.list?.accounts?.find(item => item.id === id)
        if (data?.refreshedId !== id || !refreshed || refreshed.needsRelogin === true
          || providerOf(refreshed) !== providerOf(account)) {
          throw new Error(deps.messages.unconfirmed)
        }
        ok.push({ id, name })
      }
    } catch (error) {
      failed.push({ id, error: messageOfError(error) })
    }
    deps.onProgress?.({ completed: index + 1, total: selected.length, name })
  }

  return { ok, failed, skipped }
}
