#!/usr/bin/env node
// 把「CI 真跑」的接线钉住：触发分支、--lib 目标、缓存作用域与去掉跨仓 checkout
// 都是 fork 适配点，下次同步上游时最容易被整份覆盖回去。
import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const script = path.join(root, 'scripts', 'cargo-test.mjs')
const workflow = path.join(root, '.github', 'workflows', 'rust-tests.yml')
const command = 'cargo test --locked -p agent2api-server --lib'
const failures = []

const assert = (condition, message) => {
  if (!condition) throw new Error(message)
}
const check = (name, fn) => {
  try {
    fn()
    console.log(`✅ ${name}`)
  } catch (error) {
    failures.push(name)
    console.error(`❌ ${name}：${error.message}`)
  }
}
const run = (...args) => spawnSync(process.execPath, [script, ...args], { encoding: 'utf8' })

check('--help 打印用法并写清测试命令', () => {
  const result = run('--help')
  assert(result.status === 0, `退出码 ${result.status}`)
  assert(result.stdout.includes(command), '帮助里没有实际执行的 cargo 命令')
})

check('--dry-run 只打印命令、不启动 cargo', () => {
  const result = run('--dry-run')
  assert(result.status === 0, `退出码 ${result.status}`)
  assert(result.stdout.trim() === command, `实际输出：${result.stdout.trim()}`)
  assert(!result.stdout.includes('运行服务端测试'), '干跑仍启动了 cargo')
})

check('-- 之后的过滤参数透传给 cargo test', () => {
  const result = run('--dry-run', '--', 'toml_editing::tests')
  assert(result.status === 0, `退出码 ${result.status}`)
  assert(result.stdout.trim() === `${command} -- toml_editing::tests`,
    `实际输出：${result.stdout.trim()}`)
})

check('未知参数以非零状态退出且不启动 cargo', () => {
  const result = run('--bogus')
  assert(result.status === 2, `退出码 ${result.status}`)
  assert(result.stderr.includes('未知参数'), `stderr：${result.stderr}`)
  assert(!result.stdout.includes('运行服务端测试'), '错误路径仍启动了 cargo')
})

const yaml = readFileSync(workflow, 'utf8').replace(/\r\n/g, '\n')
const lines = yaml.split('\n')
const topLevelBlock = key => {
  const start = lines.findIndex(line => line === `${key}:`)
  assert(start >= 0, `缺少顶层 ${key}:`)
  const block = []
  for (const line of lines.slice(start + 1)) {
    if (line && /^\S/.test(line)) break
    block.push(line)
  }
  return block
}

check('push 与 PR 都只在 beta 触发', () => {
  const triggers = topLevelBlock('on')
  const pushStart = triggers.findIndex(line => line.trim() === 'push:')
  const prStart = triggers.findIndex(line => line.trim() === 'pull_request:')
  assert(pushStart >= 0 && prStart > pushStart, '缺少 push 或 pull_request 触发')
  assert(triggers.slice(pushStart, prStart).some(line => line.includes('branches: [beta]')),
    'push 分支不是 [beta]')
  assert(triggers.slice(prStart).some(line => line.includes('branches: [beta]')),
    'PR base 不是 [beta]')
  assert(!triggers.some(line => /branches:.*main/.test(line)), '触发分支里混进了 main')
})

check('并发组与缓存作用域沿用 fork 约定', () => {
  assert(yaml.includes('cancel-in-progress: true'), '缺少取消在途运行的并发设置')
  assert(yaml.includes('workspaces: desktop-tauri/src-tauri -> ../../target'),
    'rust-cache 的 workspaces 与 build.yml / warm-cache.yml 不一致')
})

check('不带入上游插件仓 checkout 与 default-members', () => {
  assert(!lines.some(line => /^\s*repository:\s*\S/.test(line)), '仍有跨仓 checkout 步骤')
  assert(!yaml.includes('default-members'), '带入了 default-members')
})

check('server 作业真的调用统一测试入口', () => {
  assert(/run: node scripts\/cargo-test\.mjs/.test(yaml), '工作流没有跑 cargo-test.mjs')
})

if (failures.length) {
  console.error(`\n${failures.length} 项校验失败`)
  process.exit(1)
}
console.log('\n全部校验通过')
