import { t } from '../i18n'
import {
  supportsTokenRefreshCore,
  tokenRefreshSkipReasonCore,
  type RefreshMessages,
  type RefreshSkipTexts,
} from './accounts-batch-refresh-core'
import type { AccountRecord } from './accounts-shared'

export function supportsTokenRefresh(account: AccountRecord): boolean {
  return supportsTokenRefreshCore(account)
}

export function tokenRefreshSkipReason(account: AccountRecord): string {
  const texts: RefreshSkipTexts = {
    desktop: t('桌面端实时登录态请在原客户端更新'),
    relogin: accountReloginText(account),
    unsupported: t('该账号不支持刷新 Token 或未提供刷新凭证'),
  }
  return tokenRefreshSkipReasonCore(account, texts)
}

export function tokenRefreshMessages(): RefreshMessages {
  return {
    unavailable: t('Token 刷新接口不可用，请重新打开页面'),
    notFound: t('账号不存在，请刷新后重试'),
    unconfirmed: t('Token 刷新结果未确认，请刷新后核对账号状态'),
  }
}

export function accountReloginText(account: AccountRecord): string {
  return account.needsReloginReason?.trim()
    || t('登录授权已失效，请重新登录；自动续期无法恢复已过期或已撤销的刷新令牌。')
}
