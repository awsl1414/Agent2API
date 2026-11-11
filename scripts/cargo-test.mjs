#!/usr/bin/env node
// 本地与 CI 共用同一条 cargo 命令：两边跑不同的测试集时，CI 的绿灯说明不了本地行为。
// -p agent2api-server --lib 是仓内实际结构 —— workspace 根包（桌面壳）没有测试，
// 服务端单测全在 server/ 这个 crate 的 lib 里；不加 --lib 会连带编出 bin 目标。
import { spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const cwd = path.join(root, 'desktop-tauri', 'src-tauri')
const usage = `用法：node scripts/cargo-test.mjs [选项] [ -- <cargo test 参数> ]

运行 cargo test --locked -p agent2api-server --lib（服务端 lib 单测）。

选项：
  -h, --help     显示本帮助
  --dry-run      只打印 cargo 命令，不执行
  -- <参数>      额外参数透传给 cargo test（如测试名过滤）`

let dryRun = false
let passthrough = []
for (let index = 0; index < process.argv.length - 2; index += 1) {
  const arg = process.argv[index + 2]
  if (arg === '--') {
    passthrough = process.argv.slice(index + 3)
    break
  }
  if (arg === '-h' || arg === '--help') {
    console.log(usage)
    process.exit(0)
  }
  if (arg === '--dry-run') {
    dryRun = true
    continue
  }
  // 静默转发未知选项会让拼错的开关变成 cargo 参数，错误要等到几分钟后才暴露。
  console.error(`未知参数：${arg}\n${usage}`)
  process.exit(2)
}

const args = ['test', '--locked', '-p', 'agent2api-server', '--lib',
  ...(passthrough.length ? ['--', ...passthrough] : [])]
if (dryRun) {
  console.log(['cargo', ...args].join(' '))
  process.exit(0)
}

// 清掉开发机上残留的网关环境变量：测试读写的是配置目录，落到真实家目录会改用户数据，
// 带进 CI 的 AGENT2API_* / WORKBUDDY_* 也会让同一份测试在两处跑出不同结果。
const home = mkdtempSync(path.join(tmpdir(), 'agent2api-cargo-'))
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
  !/^(AGENT2API_|WORKBUDDY_)/i.test(key)))
env.AGENT2API_PROXY_HOME = home
env.WORKBUDDY_PROXY_HOME = home
env.NO_PROXY = 'localhost,127.0.0.1,::1'
env.no_proxy = env.NO_PROXY

console.log('运行服务端测试（使用独立临时目录）')
const child = spawn('cargo', args, { cwd, env, stdio: 'inherit' })
let cleaned = false
const cleanup = () => {
  if (cleaned) return
  cleaned = true
  try { rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }) }
  catch (error) { console.error(`临时目录未完全清理：${home}（${error.message}）`) }
}
child.once('error', error => {
  console.error(`无法启动 cargo：${error.message}`)
  cleanup()
  process.exitCode = 1
})
child.once('close', code => {
  cleanup()
  process.exitCode = code ?? 1
})
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.once(signal, () => child.kill(signal))
}
