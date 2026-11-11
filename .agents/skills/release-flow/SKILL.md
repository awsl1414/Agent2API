---
description: Agent2API 仓库的协作与发版规范 —— 分支落脚点、合并 PR 与回复 Issue 的署名、版本号递增、正式版与 beta 预发布流程、仓库与构建链路（仓内直构）、镜像发布、发版验收与排错。凡是涉及切分支、给 PR 选 base、合并 PR、回 Issue/PR、改版本号、写更新日志、打 tag、挂 GitHub Release、发 beta 版、改 CI 工作流、换仓库、排查发版失败时，都先按本 Skill 执行 —— 即使对方没提「发版」「分支」这类字眼。
---

# Agent2API 协作与发版规范

覆盖两类工作：**日常协作**（分支落脚点、PR 署名、Issue 落款，第 1–3 节）与**发版**（版本号、更新日志、tag、Release、镜像、验收，第 4–7 节），仓库与构建链路见 0.5 节，排错见第 8 节。

## 0. 两条人工红线（必须停下等确认）

下列两件事**必须由人先审过、明确点头**，代理才能继续；可以准备草稿、可以列依据，不能自己拍板往下走：

1. **发版**：进入发版流程（改版本号、建发布提交、推 main、打 tag、挂 Release）之前，先把版本号与「这次要发什么」讲清楚，等用户确认。**未经确认不得推任何分支或 tag** —— tag 一旦推出去就会触发构建、产出公开的安装包与镜像，收不回来。
2. **更新日志**：写好的日志（发布提交的提交信息，同时也是 GitHub Release 正文）必须先给用户逐条过目。用户要求改措辞、增删条目、调整分类前缀时，按反馈改完要**再确认一次**，不能因为「改得不多」就直接提交。

一次发版里两条同时适用：既要有版本号的确认，也要有日志的确认；用户说「继续」之后才动手。

## 0.5 仓库与构建链路（fork 现状：仓内直构）

**本 fork（`awsl1414/Agent2API`）不走上游的三仓架构**，安装包、服务端包与镜像都在本仓库由
`.github/workflows/build.yml` 和 `.github/workflows/docker.yml` 直接构建，并挂到本仓库的
Releases 与 GHCR。上游那套「源码仓只派发 → `Agent2Builder/agent2api-releases` 编译 →
`Agent2plugin/Agent2plugin` 索引指路」在这里没有对应凭据，也没有对应产物，**不要照搬**。

| 仓库 | 角色 | 里面有什么 |
|---|---|---|
| `awsl1414/Agent2API` | 源码 + 构建 + 发布 | 代码、文档、`build.yml`（桌面端与服务端）、`docker.yml`（镜像）、Releases 与 GHCR 包 |
| `awsl1414/Agent2plugin` | 索引仓库 | `index.json`（插件包在哪）与 `app.json`（桌面端安装包在哪），其 `app.repo` 指向本仓库 |
| `awsl1414/Agent2Providers` | 插件构建仓库 | 插件包五个平台的发布，不在本仓库的 Release 里 |

### 一次 tag 的完整链路

```
git push origin vX.Y.Z
  ├─ build.yml
  │    ├─ meta        解析 ref 与版本号（dispatch 补发时先确认 tag 存在）
  │    ├─ macos       universal dmg
  │    ├─ windows     NSIS exe
  │    ├─ linux-desktop  deb + AppImage
  │    ├─ linux-server   amd64 / arm64 服务端包（tar.gz + sha256）
  │    └─ release     六份附件齐了才挂 Release，按 tag 附注生成正文
  └─ docker.yml
       └─ image       amd64 / arm64 → ghcr.io/awsl1414/agent2api
```

两个工作流都监听 `v*` tag，也都能 `workflow_dispatch` 对已有 tag 补发，所以发布与
镜像互不阻塞：镜像失败只重跑 `docker.yml`，不必重建六个平台的包。

### 镜像

- **只推 GHCR**：`ghcr.io/awsl1414/agent2api`，用内置 `GITHUB_TOKEN` 登录，不需要任何
  secret。Docker Hub 只有配了 `DOCKERHUB_USERNAME` / `DOCKERHUB_TOKEN` 才推，镜像名跟着
  凭据用户名走。
- **`latest` 只跟正式版**：预发布 tag（带 `-`）出 `<版本>` 与滚动 `:beta`，不移动
  `:latest`；手动 `workflow_dispatch` 不填版本时只出 `:dev`。
- ⚠️ **GHCR 新包默认私有**：第一次推送后要到包设置里手工改成 Public，否则别人 pull 会 401。
- 镜像的 `AGENT2API_SELF_UPDATE` 默认是 1，容器内一键更新按 `agent2api-server-linux-<arch>.tar.gz`
  在 Release 资产里找包；`build.yml` 的 `linux-server` job 就是这份包的唯一来源，删了它
  容器内更新会停在「本次发布没有提供容器内更新包」。

### 本 fork 暂不发布的产物

| 产物 | 状态 | 影响 |
|---|---|---|
| `agent2api-server-windows-amd64.zip` | 不构建 | Windows Server 用户需从上游 Releases 取包或自行编译；六份安装文档已注明 |
| Android APK / iOS IPA | 不构建 | 手机端只能用浏览器 PWA（`/m`） |
| 桌面端代码签名 | 没有证书 | macOS 首次打开要手动放行、Windows 过 SmartScreen，Release 正文有提示 |

### 换仓库（迁到别的账号 / 自有域名镜像时）

按顺序做，**顺序错了会让用户收不到更新**：

1. 改本仓库的 `git remote` 与 `.github/workflows/*.yml` 里的 `ghcr.io/<owner>/...`；
2. 改**索引仓库**的 `app.json`：`app.repo` 指向新仓库。桌面端最多 30 分钟后自动切过去
   （`core::update::index` 有节流），用户不必做任何操作；
3. 改**索引仓库**的 `index.json`：`plugins[].source.repo` 指向新的插件构建仓库；
4. 改源码仓库里这三处常量，让「索引读不到」时的兜底也指向新地方：
   - `desktop-tauri/src-tauri/server/src/server/core/update/version.rs` 的 `DEFAULT_REPO`
   - `desktop-tauri/src-tauri/server/src/server/core/plugin_host/update.rs` 的 `FALLBACK_REPO`
   - `scripts/release.sh` 的 `BUILD_REPO` 默认值（现在从 origin 推导）

**为什么第 2 步最关键**：已经装好的旧版本二进制里烧死的是旧仓库名，只有索引能把它们领到
新地方。索引改晚了，用户手上那个版本就永远卡在旧地址。

`app.json` 的格式（`schema` 目前只有 1）：

```json
{
  "schema": 1,
  "updated": "2026-11-08",
  "app": { "repo": "awsl1414/Agent2API" }
}
```

`app.repo` 必须是 `owner/repo` 形式；写成 URL 或带空格会被当成无效值，静默回落到内置默认值。

## 1. 分支落脚点：本 fork 只有 main

| 分支 | 角色 | 什么时候动它 |
|---|---|---|
| `main` | 唯一分支：日常改动、PR 合并、发版都在这里 | 全部 |

- **日常改动直接提交到 `main` 并推送**；PR 的 base 选 `main`。
- 没有 `beta` 集成分支：上游那套「beta 集成 / main 只在发版时动」在这里不成立，别为了
  对齐上游凭空建一条 beta —— 两条分支会立刻分叉，还得靠合并提交往回捞。
- beta 版（预发布）只是**版本号带 `-` 后缀的 tag**，分支不变，见第 6 节。

## 2. 合并 PR：保留贡献者署名

合并贡献者的 PR 时，提交作者（author）必须还是贡献者本人 —— 这直接决定对方能不能进仓库首页 Contributors 名单、贡献图里有没有这一笔。署名是贡献者应得的东西，不该因为合并方式丢掉。

GitHub 的归属规则只有一条：**提交头里的 author 邮箱出现在某个账号已验证的邮箱列表里，这次提交就算那个账号的**；否则记成匿名条目，不进 Contributors。另有两条口径：统计只认默认分支（本仓库 `main`），合并提交与空提交不计入。

- **默认用 GitHub 网页上的合并按钮**（`Merge pull request` / `Squash and merge` / `Rebase and merge`）：三种都会保留原作者为 author；squash 时 PR 里的多位作者会一并进 co-author 名单。
- **禁止「本地摘取重提」**：把对方的改动拷过来自己提交（本地 `git merge --squash` 后自己 commit、重抄一遍改动、或改了 author 再提交）都不行 —— 这样 author 是合并者，贡献者的署名在 GitHub 侧完全不可见。本仓库历史上出现过这种情况，别再来一次。
- **确实要在本地处理时**（有冲突要解、要顺带调整），必须把署名带上：单个原作者用 `git commit --author="原作者 <原作者邮箱>"`；一个提交里有多个人的工作，在提交信息末尾加 trailer `Co-authored-by: 名字 <邮箱>`。
- **摘除 AI 的 co-author trailer**：贡献者提交信息末尾的 `Co-Authored-By: Claude Code <…>` 这类 AI trailer，合并时一律删除 —— GitHub 会把它渲染成共同作者，进了 `main` 就出现在 Contributors 名单里，而 AI 不是贡献者。只删 trailer 行本身，author 与其余提交信息逐字保留；正文里对 AI 产品的**技术性提及**（「支持 Claude Code 客户端」「Claude 兼容端点」之类）与此无关，不动。摘除动的是已合入分支的历史，趁合并后、发版前尽早做（`git filter-branch --msg-filter` 摘行后 force-push），拖到发版后就得重写 `main`。
- **邮箱必须能对上账号**：用对方 GitHub 账号里**已验证**的地址；对方想保密就用 GitHub 给的 `ID+用户名@users.noreply.github.com`（在对方账号的 Emails 设置里查）。内网地址、`user@机器名` 这类邮箱永远关联不上账号，提交会被记成匿名。
- 合并后顺手核对一次：`git log --format='%an <%ae>' -1 <合并提交>`，或在 PR 页面看 commit 旁的头像是否指向原作者。

## 3. 回复 Issue / PR：标明 AI 来源

**由 AI 代理发出（含代笔）的每一条 Issue / PR 回复，末尾都要单独一行写明来源。** 收件人有权知道自己在跟谁说话 —— 与第 2 节的署名是同一条道理：**谁说的写谁**。

固定落款，逐字照用（别改写成「机器人」「助手」这类含糊说法）：

> 🤖 本条回复由 AI Agent 生成

- **位置**：整条回复的最后一行，单独成段（前后各留一个空行）；不要塞进正文段落、列表项、引用块或代码块里 —— 引用别人的话时，落款落在自己的话之后。
- **英文场合**（外部贡献者的 PR / Issue）在中文那行下面再补一行 `Generated by an AI agent.`，说的是同一件事，两边都不省。
- **代理自己新建的 Issue / PR**，正文末尾同样加。
- **代笔也算**：人在对话里让代理转达（哪怕逐字）一段话，落款照加 —— 它描述的是「这条回复是谁发出去的」，不是「谁构思的」。
- 只约束 Issue / PR **回复**（评论、review 意见、关闭 / 合并说明）。提交信息不追加这一行，署名口径见第 2 节。

## 4. 版本号

- 版本号形如 `X.Y.Z`（如 `2.7.2`），日常小版本「递增 0.01」= 末位 +1（如 `2.7.8` → `2.7.9`）。
- **递增幅度以用户要求为准**：默认用上面的「递增 0.01」；用户另有要求时（如「这次递增 0.1」）一律听用户的，按用户指定的幅度算出新版本号，不要自作主张套用默认幅度。
- **版本号要同步改 6 处**（缺一处会导致安装包与显示版本对不上）：
  1. `package.json`（根）
  2. `desktop-tauri/package.json`
  3. `desktop-tauri/src-tauri/Cargo.toml`
  4. `desktop-tauri/src-tauri/tauri.conf.json`
  5. `desktop-tauri/src-tauri/server/Cargo.toml`
  6. `desktop-tauri/mobile/package.json` —— 手机端安装包的版本号都从这里读：Android 的 `versionName` / `versionCode` 由 `android/variables.gradle` 读它（`versionCode` 由 semver 折算，只增不减，见该文件注释）；iOS 的 `MARKETING_VERSION` / `CURRENT_PROJECT_VERSION` 由上游构建仓的 ios job 读取。**本 fork 不构建手机端包**（见 0.5 节），这一处仍要跟版本走，否则 PWA 与源码头信息对不上
- **改完第 6 处，必须顺手把 `desktop-tauri/mobile/package-lock.json` 根部的 `version` 也改成同一个值**（文件顶层的 `"version"` 与 `packages[""].version` 两处）。`npm ci` 对这两处不一致是**静默容忍**的（实测不会报错，也不会自动改），漏改 CI 照绿，但仓库里会留下版本号不一致的 lock 文件，下次谁跑 `npm install` 又会把它改回来，平白多出一个无关 diff。桌面端那几处没有这个问题（`desktop-tauri/package-lock.json` 的版本本来就落后，不随发版改）。
- 应用内「关于」与更新检查读的是 `env!("CARGO_PKG_VERSION")`（第 5 处），改完跑一次 `cargo check` 让 `Cargo.lock` 跟着刷新。
- 手机端界面目前**不显示**包自身版本号（只显示所连网关的版本）；PWA 形态的版本只能从仓库源码确认。
- **顺手过一遍安装文档**：安装说明已经从 README 搬进 `docs/install.md`（README 只留速查表 + 一个链接），另有 5 份译文 `docs/install.{en,zh-Hant,ja,ko,pt-BR}.md`，六份内容一一对应。凡是这次改动碰到了**安装 / 升级 / 卸载步骤、产物体名、默认端口、环境变量、安装脚本参数**，六份都要同步改；README 速查表受影响时也一起改。
- **README 语言落点**：根 `README.md` 是**英文**（GitHub 默认展示），简体中文在 `docs/README.zh-CN.md`，其余为 `docs/README.{zh-Hant,ja,ko,pt-BR}.md`。中文仍是文案源语言，支持矩阵等六份要同步的内容改完记得回填英文根 README。

## 5. 正式版发版流程

按顺序执行；第 0 节的两条红线贯穿全程。

```bash
# 1. 收干净工作区（tag 里的代码就是发出去的代码）
git status --short        # 必须为空；有改动就先提交或清理
cargo check               # 或前端有改动时做一次构建自检

# 2. 切到 main（本 fork 只有 main 一条分支，日常改动也直接落 main）
git switch main && git pull
git status --short        # 再确认一次

# 3. 六处版本号一起改（默认末位 +1，幅度听用户）；改完刷新 Cargo.lock
cargo check

# 4. 写发布提交：提交信息 = 更新日志
git commit -am "…更新日志…"

# 5. 推 main、打 tag、推 tag
git push origin main
git tag vX.Y.Z
git push origin vX.Y.Z

# 6. 看进度（就在本仓库，两个 run 并行）
gh run list --limit 4
gh run watch <run-id> --exit-status
```

**发布提交的提交信息 = 更新日志**。`build.yml` 的 `release` job 读 tag 附注（轻量 tag 回落成
提交信息）解析成 Release 正文，所以**打 annotated tag 并把更新日志写进附注**最稳妥：

```bash
git tag -a vX.Y.Z -m "$(cat 更新日志.txt)"
```

- 格式沿用历史版本的编号清单，每行一条，带分类前缀：`新增：` / `优化：` / `修复：` /
  `变更：` / `移除：` / `插件：` / `文档：`，最后一条固定 `版本：X.Y.Z → X.Y.Z+1`；
- 内容取「上个 tag 以来的全部提交」综合整理（`git log vX.Y.Z..HEAD --oneline`），纯 CI /
  临时的调试提交归并成一条流程性描述即可；
- 本次没有代码改动时用空提交承载：`git commit --allow-empty -F 更新日志.txt`；
- 更新日志草稿与历史发布说明统一写在 `docs/release-notes/<版本号>.md`（如
  `docs/release-notes/3.4.3.md`），不要在项目根目录新建 `.release-notes-*.md` ——
  `docs/release-notes/` 已在忽略列表里，新增文件不必再加忽略规则。

### 两个工作流的产出

| 工作流 | 产出 | 说明 |
|---|---|---|
| `build.yml` | 六份附件：`Agent2API_<版本>_universal.dmg`、`_x64-setup.exe`、`_amd64.deb`、`_amd64.AppImage`、`agent2api-server-linux-{amd64,arm64}.tar.gz`，各带 `.sha256` | 六个 job 全绿后 `release` job 才跑；它核对四份桌面端包名里的版本号与 tag 一致、两份服务端包的 sha256 自洽，再幂等挂 Release |
| `docker.yml` | `ghcr.io/awsl1414/agent2api:<版本>` 与 `:latest`，amd64 / arm64 双架构 | 正式版推 `<版本>` + `:latest`；beta 推 `<版本>` + `:beta`，**不移动 `:latest`** |

两个工作流互不阻塞：镜像失败只重跑 `docker.yml`，不必重建六个平台的包。

```bash
# 补发镜像（对已存在的 tag，不填 version 时对当前分支出 :dev）
gh workflow run docker.yml -f version=X.Y.Z
# 补发安装包（对已存在的 tag 重跑整个 build.yml，未变的部分命中缓存）
gh workflow run build.yml -f version=X.Y.Z
```

**附件清单是硬校验**：`build.yml` 的 `release` job 要求恰好 4 个安装包 + 2 个服务端包；
少任何一个都直接失败，不会挂出半套 Release。上传逐文件 `--clobber`：gh 批量上传遇到
「附件已存在」会报 HTTP 422 中断整批，逐个覆盖让重跑天然幂等。

本地 `scripts/release.sh` 保留为**应急入口**，用于自动流程不可用或旧版本补发：

```bash
bash scripts/release.sh vX.Y.Z <run-id>
```

- 目标仓库默认从 `origin` 推导（本 fork 就是本仓库），可用 `BUILD_REPO=` 覆盖；
- 脚本找的是名为 **`build`** 的工作流；
- 更新日志仍从本仓库取（`git log -1 --format=%B "$TAG"`），所以要在源码仓库目录里跑；
- 脚本会清空当前目录的 `dist/`，**使用前检查并保护已有产物**，建议在独立工作区运行。

### GHCR 与包可见性

镜像只用内置 `GITHUB_TOKEN` 推，不需要配 secret。两个注意事项：

- **新包默认私有**：第一次推送后要到 `https://github.com/users/awsl1414/packages/container/agent2api/settings`
  把可见性改成 Public，否则别人 `docker pull` 会 401。API 改不了这个默认值。
- **必须显式写镜像名**：工作流跑在本仓库，可以用 `${GITHUB_REPOSITORY}`，但换成别的账号
  构建时那个变量会解析成构建仓库的名字，镜像就推到别人的命名空间下去了。`docker.yml`
  用的是同一套写法，迁移时一并核对。

## 6. beta 版（预发布）

应用里有「设置 → 软件更新 → 更新设置 → **加入 beta 版**」开关：**打开的人才收到 beta
更新，没打开的人只看正式发布。**

beta 版不是另一条流水线，而是第 5 节流程的变体 —— 唯一结构差别是**版本号与 tag 带预发布
后缀**，提交、tag、Release、验收逐字照旧。第 0 节的两条红线同样适用。本 fork 只有 `main`
一条分支，所以 tag 就落在 `main` 的提交上。

```bash
# 1. 在 main 上动手
git switch main && git pull
git status --short          # 必须为空

# 2. 六处版本号一起改成「与 tag 逐字一致」的形态：
#    tag 打 v3.4.3-beta.2 → 六处都写 3.4.3-beta.2（清单见第 4 节）
cargo check                 # 顺带刷新 Cargo.lock

# 3. 提交并推 main（提交信息 = 这次的 beta 更新日志，格式同第 5 节）
git commit -am "…beta 版日志…"
git push origin main

# 4. 打 annotated tag 并推
git tag -a v3.4.3-beta.2 -m "…同一份日志…"
git push origin v3.4.3-beta.2

# 5. 看两个工作流（build 与 docker），都在本仓库
gh run list --limit 4
```

验收就四条：Release 显示 **Prerelease**、六份附件齐全、GHCR 上 `:latest` 的更新时间
**没有**跟着变、`:beta` 与本版标签指向同一份清单。

**三处不能错**，每处错了都有明确症状：

1. **六处版本号漏了后缀**（写成 `3.4.3`）：比较是按 semver 做的，运行版本 `3.4.3` 对线上
   tag `3.4.3-beta.2` 会判成「当前比线上新」，**beta 用户永远收不到更新**。六个文件都接受
   带后缀的版本号（Cargo / npm / tauri.conf.json 都认），照 tag 抄即可。
2. **Release 漏标 pre-release**：`build.yml` 按 `版本号带 -` 自动标，与
   `core::update::version::is_prerelease_version` 同一条判据；版本号写错就标错。
3. **`:latest` 被移动**：`docker.yml` 只看版本号里有没有 `-` 决定推 `:beta` 还是 `:latest`，
   版本号漏后缀会同时把 `:latest` 换掉 —— 正式版用户会在没做任何选择的情况下收到 beta。

后续常见三件事：

- **再发一个 beta**：核心号不变、递增序号（`3.4.3-beta.3`，六处一起改，上面那套命令照走）。
  预发布序号能分出先后（`beta.3` > `beta.2`），已经在 beta 上的人照样收到更新 —— 所以
  beta 之间也要保持「tag 与版本号逐字一致」。
- **转正**（发 `3.4.3` 正式版）：按第 5 节走，六处版本号去掉后缀。**beta 用户不会自动接回
  正式版**：开关开着的人只看预发布版（`pick_listed_release` 按「当前跑的是不是 beta 版」
  筛），转正的 `3.4.3` 对他们不算更新 —— 要留在 beta 频道的人等下一个 beta；想回正式版
  由用户自己关开关，关掉后只在**版本号高于他手上那个 beta** 的正式版发布时才收到更新。
- **「打开了却没有反应」**：仓库里一个预发布 Release 都没有时，打开这个开关也只会看到
  正式版 —— 第一次 beta 发出去之后才有效果。

## 7. 验收清单

下面的 `gh` 命令都在本仓库目录里跑即可（`gh` 会自动认 origin）；手动执行前先设代理，见 8.3。

- [ ] **两个 run 全绿**：`gh run list --limit 4` —— 应看到 `build` 与 `docker` 两个 run，
  前者六个 job（`meta` / `macos` / `windows` / `linux-desktop` / `linux-server` / `release`）
  全绿；
- [ ] **Release 附件齐全（12 个文件 = 6 份产物 + 6 份校验和）**：
  `gh release view vX.Y.Z --json assets --jq '.assets[].name'` 应包含
  `Agent2API_<版本>_x64-setup.exe`、`_universal.dmg`、`_amd64.deb`、`_amd64.AppImage`、
  `agent2api-server-linux-amd64.tar.gz`、`agent2api-server-linux-arm64.tar.gz`，各带 `.sha256`；
- [ ] **正文齐全**：Release 正文有「概述」与分节条目，末尾「说明」提到未签名提示与更新通道；
  预发布版还应显示 **Prerelease** 标记；
- [ ] **服务端包内容对**：`tar -tzf agent2api-server-linux-amd64.tar.gz` 应看到
  `agent2api-server`、`ui/`、`install.sh`、`agent2api.service`、`agent2api.env.example`；
- [ ] **GHCR 镜像可拉**：`docker pull ghcr.io/awsl1414/agent2api:<版本>` 成功，
  `amd64` / `arm64` 两种架构都在；首次发布新包后确认包可见性已改为 Public；
- [ ] **正式版**：GHCR 上 `<版本>` 与 `latest` 的更新时间一致；
- [ ] **beta 版**：GHCR 上 `<版本>` 与 `beta` 指向同一份清单，`latest` 的时间**没有**跟着变；
- [ ] **更新检查真的能查到**（这条最容易漏）：Release 存在，且索引仓库 `awsl1414/Agent2plugin`
  的 `app.json` 里 `app.repo` 指向 `awsl1414/Agent2API`。两者对不上时，面板会显示「已是最新」；
- [ ] **容器内更新可用**：在 Docker 部署的面板里点「设置 → 软件更新 → 下载并更新」，能取到
  服务端包并通过校验；取不到时会提示「本次发布没有提供容器内更新包」，多半是 `linux-server`
  job 没跑或包名改了；
- [ ] **安装包「关于」页版本号与 tag 一致**；
- [ ] **安装文档对得上实际产物**：`docs/install.md` 与 5 份译文的产物名、端口、命令、环境变量
  跟这次 Release 一致；README 里的速查表同理。


## 8. 已知坑与排查

### 8.1 secrets

本 fork 的构建**不需要任何 secret**：安装包与镜像都用内置 `GITHUB_TOKEN`，权限在
`build.yml` 的 `permissions: contents: write` 与 `docker.yml` 的 `packages: write` 里声明。
可选的只有 Docker Hub 那一对：

| secret | 用途 | 缺了会怎样 |
|---|---|---|
| `DOCKERHUB_USERNAME` / `DOCKERHUB_TOKEN` | 顺带推 Docker Hub | 整步跳过，工作流照绿，只推 GHCR |

GitHub 的 secret 值读不回来（设计如此），换账号迁移时要由人重新填。

### 8.2 镜像

- **GHCR 新包默认私有**：首次推送后到包设置里改 Public，否则别人 `docker pull` 会 401
  （API 改不了这个默认值）。
- **镜像名不要用 `${GITHUB_REPOSITORY}` 之外的隐式来源**：工作流跑在本仓库时它没问题，
  但换账号构建会解析成构建仓库的名字，镜像就推到别处去了。`docker.yml` 显式写
  `ghcr.io/${GITHUB_REPOSITORY}`，迁移时一并核对。
- **包要关联主仓库**：`docker.yml` 在 `labels:` 里显式写
  `org.opencontainers.image.source`，这样 GHCR 的包页面才指回源码仓库 —— 不写的话它认的是
  「工作流所在仓库」，跨账号时连不上。

### 8.3 代理

本机直连 github.com 常被重置，gh 与 git 都要带代理：

```bash
export HTTPS_PROXY=http://127.0.0.1:7897 HTTP_PROXY=http://127.0.0.1:7897
```

`gh` 是 Go 程序，**不读** `~/.gitconfig` 里的 `http(s).proxy`，只认这两个环境变量。代理没开时
gh 会立刻报 `proxyconnect tcp: ... connection refused` 而不会回退直连 —— 与本机 `git push`
的表现一致。首次用之前跑 `~/.claude/scripts/check-proxy.sh` 确认。

### 8.4 推含工作流改动的提交被拒

报错 `refusing to allow an OAuth App to create or update workflow ... without workflow scope`。
解决：`gh auth switch --user <账号>` 切到目标账号，再 `gh auth refresh --scopes workflow`。
`gh auth refresh` **没有 `--user` 参数**，只作用于当前活动账号；不先 switch 就会出现
「浏览器里授权的是 A，refresh 的是 B」。

### 8.5 其它

- **协议库按 tag 引用插件仓库**：`desktop-tauri/src-tauri/server/Cargo.toml` 里
  `agent2api-provider-protocol` 指向 `ProvidersPlugin/Agent2Providers` 的某个 tag。插件新增
  协议字段时，要等插件仓库打了新 tag 并发布后，这里才能跟上；本地联调若需要临时改协议，
  用 `.cargo/config.toml` 的 `[patch]` 指向本地路径，**但绝不能把那段提交**。
- **服务端包名不能随手改**：`agent2api-server-linux-<arch>.tar.gz` 是
  `update/version.rs` 的 `server_bundle_name()` 与 `build.yml` 的 `linux-server` job 之间的
  逐字约定，改名要三处一起改（还有安装文档六份）。
- **打包顺序**：tar.gz 不能「先 tar 再 -r 追加」，gzip 流无法原地追加，会报
  `Cannot update compressed archives`；先把条目归拢到暂存目录再一次压好。
