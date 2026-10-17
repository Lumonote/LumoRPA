# 发布与安装包 / Release & Packaging

> 一句话：`main` 分支每次推送都自动产出**可安装的**包（优先 Windows 与 macOS）；`v*` tag 产出**签名的**正式 Release。
> 本文只管「产物从哪来、叫什么、怎么拿到」，构建细节见 [`../apps/desktop/README.md`](../apps/desktop/README.md)。

## 1. 两条通道

[`release.yml`](../.github/workflows/release.yml) 是一个 workflow、两条通道，共用同一套打包脚本，
所以**两条通道能产出的东西是一样的**，区别只在签名与保留期：

| | 触发 | 产物去向 | 签名 | 产物保留 |
| --- | --- | --- | --- | --- |
| dev 通道 | push `main` | 滚动预发布 `dev-latest`（`prerelease: true`，**不是** latest） | ❌ 未签名、未公证 | 14 天 |
| 正式通道 | push `v*` tag | 同名 GitHub Release，各平台各自附产物 | ✅ 需要 secrets；缺任一项**直接失败** | 90 天 |

两条通道都先过 `production-gate`（agent / voice / desktop / 前端测试 + clippy）。门禁红 → 不发布。

`workflow_dispatch` 手动触发走 dev 通道语义（发布 `dev-latest`）。
`concurrency.group` 按 ref 收敛：同名 ref 的新构建会取代旧构建，但 **tag 构建取消失效**——已经开始的正式发布必须跑完。

## 2. 产物清单

| 平台 | 桌面安装包 | CLI 归档 |
| --- | --- | --- |
| **Windows x86_64**（优先） | NSIS `.exe` **和** WiX `.msi`（两者可能同名，发布前分别断言存在） | `lumorpa-<ver>-windows-x86_64.zip` |
| **macOS universal**（优先） | `.dmg`——**单文件**覆盖 Intel + Apple Silicon | `lumorpa-<ver>-darwin-universal.tar.gz` |
| Linux x86_64 | `.deb` / `.rpm` / `.AppImage` | `lumorpa-<ver>-linux-x86_64.tar.gz` |
| Linux aarch64（麒麟 ARM） | `.deb` / `.rpm` | `lumorpa-<ver>-linux-aarch64.tar.gz` |
| loongarch64（龙芯） | —（无桌面包） | `lumorpa-<ver>-linux-loongarch64.tar.gz`（best-effort 交叉编译，`continue-on-error`） |

CLI 归档另有 `.sha256` 校验文件，校验前请用 `shasum -c`。

**dev 构建没有 `.sig` updater 产物**：main 上没有 `LUMO_UPDATER_PUBKEY`，
`scripts/build-desktop.sh` 会跳过 updater 产物，而不是生成一份指向未签名负载的自动更新清单。

## 3. 怎么下载

1. **滚动预发布（固定链接，最省事）**：<https://github.com/Lumonote/LumoRPA/releases/tag/dev-latest>
   该 tag 每次 main 推送被重建覆盖，链接恒定、内容最新。
2. **Workflow 运行页 → Artifacts**：[`release` workflow](https://github.com/Lumonote/LumoRPA/actions/workflows/release.yml)
   最近一次 main 运行底部。**14 天后过期**。
3. **CLI**：

   ```bash
   gh run download --repo Lumonote/LumoRPA \
     --branch main --workflow release.yml \
     --pattern 'lumorpa-*' --dir ./lumorpa-dev
   ```

## 4. 未签名意味着什么

`dev-latest` 是**非官方、未签名的开发快照**，macOS 会弹 Gatekeeper、Windows 会弹 SmartScreen。
这是预期行为，说明包确实没签名，不是包坏了。

- macOS：Finder 里**右键（Control + 点击）应用 → 打开**，再确认一次（直接双击会被拦）。必要时
  `xattr -dr com.apple.quarantine /Applications/LumoRPA.app`——只在你信任该构建时使用。
- Windows：SmartScreen 提示 → **更多信息 → 仍要运行**。

**受支持的构建 = 版本化 `v*` tag 的正式 Release。** 需要签名包就不要用 `dev-latest`。

## 5. 本地复现 CI 打包

```bash
scripts/package-cli.sh                                  # 只打 CLI 归档（代价最低）
scripts/package-release.sh universal-apple-darwin app,dmg   # macOS：桌面包 + CLI 归档
scripts/package-release.sh x86_64-pc-windows-msvc nsis,msi  # Windows（在 Windows 构建机上）
```

打包脚本**失败即失败**：产物缺失或为空一律非零退出，不产出「看起来成功、装不了东西」的包。
`.github/workflows/ci.yml` 的 `bundle-smoke` 任务在 PR 上便宜地跑一遍 CLI 打包路径，
桌面打包则由 `release.yml` 在 main 上覆盖。
