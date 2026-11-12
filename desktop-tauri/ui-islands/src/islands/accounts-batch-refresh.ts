import { displayNameOf, providerOf } from './accounts-domain'
import { findAccount } from './accounts-store'
import { errorMessage, shared, type BatchAccountsResult } from './accounts-shared'
import { tokenRefreshMessages, tokenRefreshSkipReason } from './accounts-authorization'
import { runBatchTokenRefresh, type RefreshProgress } from './accounts-batch-refresh-core'

export type BatchTokenRefreshProgress = RefreshProgress

export async function refreshAccountTokens(
  ids: string[],
  onProgress: (progress: BatchTokenRefreshProgress) => void,
): Promise<BatchAccountsResult> {
  const bridge = shared().workbuddyDesktop
  return runBatchTokenRefresh(ids, {
    findAccount,
    refreshAccountToken: bridge?.refreshAccountToken ? (id: string) => bridge.refreshAccountToken(id) : undefined,
    nameOf: displayNameOf,
    providerOf,
    skipReason: tokenRefreshSkipReason,
    onProgress,
    messages: tokenRefreshMessages(),
    errorMessage,
  })
}
