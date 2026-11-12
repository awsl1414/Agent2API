import test from 'node:test'
import assert from 'node:assert/strict'
import { setTimeout as delay } from 'node:timers/promises'
import {
  BATCH_ACTION_VALUES,
  runBatchTokenRefresh,
  supportsTokenRefreshCore,
  tokenRefreshSkipReasonCore,
} from '../desktop-tauri/ui-islands/src/islands/accounts-batch-refresh-core.ts'

const MESSAGES = {
  unavailable: '刷新接口不可用',
  notFound: '账号不存在',
  unconfirmed: '结果未确认',
}

const SKIP_TEXTS = {
  desktop: '桌面端实时登录态请在原客户端更新',
  relogin: '需要重新登录',
  unsupported: '该账号不支持刷新 Token 或未提供刷新凭证',
}

function account(id, extra = {}) {
  return { id, provider: 'workbuddy', name: id, canRefresh: true, ...extra }
}

function confirmed(id, extra = {}) {
  return { refreshedId: id, list: { accounts: [account(id, extra)] } }
}

function deps(accounts, bridge, overrides = {}) {
  const map = new Map(accounts.map(item => [item.id, item]))
  return {
    findAccount: id => map.get(id) ?? null,
    refreshAccountToken: bridge,
    nameOf: item => item?.name || item?.id || '',
    providerOf: item => item?.provider || '',
    skipReason: item => tokenRefreshSkipReasonCore(item, SKIP_TEXTS),
    messages: MESSAGES,
    ...overrides,
  }
}

test('批量动作包含 refresh，并逐项刷新旧动作没有覆盖的账号', async () => {
  assert.ok(BATCH_ACTION_VALUES.includes('refresh'))
  const calls = []
  const result = await runBatchTokenRefresh(['a', 'b'], deps(
    [account('a'), account('b')],
    async id => { calls.push(id); return confirmed(id) },
  ))
  assert.deepEqual(calls, ['a', 'b'])
  assert.deepEqual(result.ok.map(item => item.id), ['a', 'b'])
  assert.deepEqual(result.failed, [])
  assert.deepEqual(result.skipped, [])
})

test('重复 id 只刷新一次', async () => {
  const calls = []
  const result = await runBatchTokenRefresh(['a', 'a', 'b'], deps(
    [account('a'), account('b')],
    async id => { calls.push(id); return confirmed(id) },
  ))
  assert.deepEqual(calls, ['a', 'b'])
  assert.deepEqual(result.ok.map(item => item.id), ['a', 'b'])
})

test('upstream_model 不参与刷新判据', async () => {
  const noCredentials = account('a', { canRefresh: false, hasRefreshToken: false, upstream_model: 'gpt-5' })
  const withCredentials = account('b', { canRefresh: true, upstream_model: 'gpt-5' })
  const calls = []
  const result = await runBatchTokenRefresh(['a', 'b'], deps(
    [noCredentials, withCredentials],
    async id => { calls.push(id); return confirmed(id) },
  ))
  assert.deepEqual(calls, ['b'])
  assert.deepEqual(result.skipped.map(item => item.id), ['a'])
  assert.equal(result.skipped[0].reason, SKIP_TEXTS.unsupported)
  assert.deepEqual(result.ok.map(item => item.id), ['b'])
})

test('统一跳过判据覆盖桌面端、重登与无刷新凭证', () => {
  assert.equal(supportsTokenRefreshCore(account('a', { desktop: true, hasRefreshToken: true })), false)
  assert.equal(tokenRefreshSkipReasonCore(account('a', { desktop: true, canRefresh: true }), SKIP_TEXTS), SKIP_TEXTS.desktop)
  assert.equal(tokenRefreshSkipReasonCore(account('b', { needsRelogin: true, canRefresh: true }), SKIP_TEXTS), SKIP_TEXTS.relogin)
  assert.equal(tokenRefreshSkipReasonCore(account('c', { canRefresh: false, hasRefreshToken: false }), SKIP_TEXTS), SKIP_TEXTS.unsupported)
  assert.equal(tokenRefreshSkipReasonCore(account('d', { canRefresh: true }), SKIP_TEXTS), '')
})

test('空选中不调用桥，也不产生进度', async () => {
  const calls = []
  const progress = []
  const result = await runBatchTokenRefresh([], deps(
    [],
    async id => { calls.push(id); return confirmed(id) },
    { onProgress: item => progress.push(item) },
  ))
  assert.deepEqual(result, { ok: [], failed: [], skipped: [] })
  assert.deepEqual(calls, [])
  assert.deepEqual(progress, [])
})

test('账号不存在记失败并继续下一项', async () => {
  const calls = []
  const result = await runBatchTokenRefresh(['missing', 'a'], deps(
    [account('a')],
    async id => { calls.push(id); return confirmed(id) },
  ))
  assert.deepEqual(calls, ['a'])
  assert.deepEqual(result.ok.map(item => item.id), ['a'])
  assert.deepEqual(result.failed, [{ id: 'missing', error: MESSAGES.notFound }])
})

test('需要重登或无刷新凭证的账号跳过且不调用桥', async () => {
  const calls = []
  const result = await runBatchTokenRefresh(['relogin', 'unsupported'], deps(
    [
      account('relogin', { needsRelogin: true, canRefresh: true }),
      account('unsupported', { canRefresh: false, hasRefreshToken: false }),
    ],
    async id => { calls.push(id); return confirmed(id) },
  ))
  assert.deepEqual(calls, [])
  assert.deepEqual(result.skipped.map(item => [item.id, item.reason]), [
    ['relogin', SKIP_TEXTS.relogin],
    ['unsupported', SKIP_TEXTS.unsupported],
  ])
  assert.deepEqual(result.ok, [])
  assert.deepEqual(result.failed, [])
})

test('桥返回缺 refreshedId 视为未确认', async () => {
  const result = await runBatchTokenRefresh(['a'], deps(
    [account('a')],
    async () => ({ list: { accounts: [account('a')] } }),
  ))
  assert.deepEqual(result.failed, [{ id: 'a', error: MESSAGES.unconfirmed }])
  assert.deepEqual(result.ok, [])
})

test('结果回读 provider 漂移视为未确认', async () => {
  const result = await runBatchTokenRefresh(['a'], deps(
    [account('a')],
    async () => confirmed('a', { provider: 'other' }),
  ))
  assert.deepEqual(result.failed, [{ id: 'a', error: MESSAGES.unconfirmed }])
})

test('结果回读需要重登视为未确认', async () => {
  const result = await runBatchTokenRefresh(['a'], deps(
    [account('a')],
    async () => confirmed('a', { needsRelogin: true }),
  ))
  assert.deepEqual(result.failed, [{ id: 'a', error: MESSAGES.unconfirmed }])
})

test('结果回读缺账号快照视为未确认', async () => {
  const result = await runBatchTokenRefresh(['a'], deps(
    [account('a')],
    async () => ({ refreshedId: 'a', list: { accounts: [] } }),
  ))
  assert.deepEqual(result.failed, [{ id: 'a', error: MESSAGES.unconfirmed }])
})

test('中途失败仍继续下一项', async () => {
  const calls = []
  const result = await runBatchTokenRefresh(['a', 'b'], deps(
    [account('a'), account('b')],
    async id => {
      calls.push(id)
      if (id === 'a') throw new Error('刷新失败')
      return confirmed(id)
    },
  ))
  assert.deepEqual(calls, ['a', 'b'])
  assert.deepEqual(result.failed, [{ id: 'a', error: '刷新失败' }])
  assert.deepEqual(result.ok.map(item => item.id), ['b'])
})

test('部分成功与跳过分别汇总', async () => {
  const result = await runBatchTokenRefresh(['ok', 'skip', 'fail', 'missing'], deps(
    [
      account('ok'),
      account('skip', { canRefresh: false, hasRefreshToken: false }),
      account('fail'),
    ],
    async id => {
      if (id === 'fail') throw new Error('桥失败')
      return confirmed(id)
    },
  ))
  assert.deepEqual(result.ok.map(item => item.id), ['ok'])
  assert.deepEqual(result.skipped.map(item => item.id), ['skip'])
  assert.deepEqual(result.failed.map(item => item.id), ['fail', 'missing'])
  assert.equal(result.failed[0].error, '桥失败')
  assert.equal(result.failed[1].error, MESSAGES.notFound)
})

test('桥调用保持串行，不同时发起多个刷新', async () => {
  const calls = []
  let active = 0
  let maxActive = 0
  const result = await runBatchTokenRefresh(['a', 'b', 'c'], deps(
    [account('a'), account('b'), account('c')],
    async id => {
      calls.push(id)
      active += 1
      maxActive = Math.max(maxActive, active)
      try { await delay(4); return confirmed(id) }
      finally { active -= 1 }
    },
  ))
  assert.deepEqual(calls, ['a', 'b', 'c'])
  assert.equal(maxActive, 1)
  assert.deepEqual(result.ok.map(item => item.id), ['a', 'b', 'c'])
})

test('并发触发时进度不串号', async () => {
  const progressA = []
  const progressB = []
  const accountsA = [account('a1', { provider: 'run-a' }), account('a2', { provider: 'run-a' })]
  const accountsB = [account('b1', { provider: 'run-b' }), account('b2', { provider: 'run-b' }), account('b3', { provider: 'run-b' })]
  const bridgeA = async id => { await delay(3); return confirmed(id, { provider: 'run-a' }) }
  const bridgeB = async id => { await delay(2); return confirmed(id, { provider: 'run-b' }) }
  const [resultA, resultB] = await Promise.all([
    runBatchTokenRefresh(['a1', 'a2'], deps(accountsA, bridgeA, {
      onProgress: item => progressA.push({ run: 'A', ...item }),
    })),
    runBatchTokenRefresh(['b1', 'b2', 'b3'], deps(accountsB, bridgeB, {
      onProgress: item => progressB.push({ run: 'B', ...item }),
    })),
  ])
  assert.deepEqual(resultA.ok.map(item => item.id), ['a1', 'a2'])
  assert.deepEqual(resultB.ok.map(item => item.id), ['b1', 'b2', 'b3'])
  assert.deepEqual(progressA.map(item => item.completed), [0, 1, 1, 2])
  assert.deepEqual(progressB.map(item => item.completed), [0, 1, 1, 2, 2, 3])
  assert.ok(progressA.every(item => item.run === 'A' && item.total === 2 && item.name.startsWith('a')))
  assert.ok(progressB.every(item => item.run === 'B' && item.total === 3 && item.name.startsWith('b')))
})

test('刷新接口缺失时抛出错误', async () => {
  await assert.rejects(
    () => runBatchTokenRefresh(['a'], deps([account('a')], undefined, { refreshAccountToken: undefined })),
    { message: MESSAGES.unavailable },
  )
})
