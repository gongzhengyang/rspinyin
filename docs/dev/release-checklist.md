# rspinyin 发布检查单

本文件是 `BUILD-P2.05.01`（tag 驱动的全自动发布流水线）的配套文档。**正常情况下不需要人工
执行任何一步**——`git push origin v0.1.0` 会触发全部流程：`gate`（全门禁 + 版本断言）→
`source`（源码 tarball）→ `packages`（矩阵并行构建三发行版包）→ `assemble`（汇总、生成
manifest、签名、自校验）→ `publish`（创建 GitHub Release）。本文件用于三种情况：发布前的
最后一次人工确认、发布失败时的定位顺序、以及首次发布前的预演。

> 流水线文件是 `.github/workflows/release.yml`。它与本文档必须保持一致：改一处必须改另一处。

---

## 1. 发布前确认（打 tag 之前）

- [ ] `main` 上的 CI 全绿（`quality` / `host-abi` / `cross-arch` / `audit` / `bench` /
      `reproducible` / `dictionary` / `size`）
- [ ] `Cargo.toml` 的 `[workspace.package] version` = 即将打的 tag（去掉前缀 `v`）。
      流水线的 `gate` 作业会用 `xtask release check-tag` 再断言一次，不一致即失败
- [ ] `packaging/fcitx5/rspinyin.conf` 与 `rspinyin-ui.conf` 的 `Version` 与之一致
      （`just check-versions`，`gate` 作业也会再跑一次）
- [ ] `packaging/debian/changelog` 有对应条目（`deb` 作业会断言包版本以 `<version>-` 开头）
- [ ] `packaging/rpm/rspinyin.spec` 的 `Version:` 与之一致（`rpm` 作业会断言包版本等于版本号）
- [ ] `packaging/aur/PKGBUILD` 的 `pkgver` 与之一致（`pkg` 作业会断言包版本以 `<version>-` 开头）
- [ ] `packaging/keys/rspinyin-signing-key.asc` 已提交，且与仓库 secret
      `RSPINYIN_GPG_PRIVATE_KEY` 是同一把钥匙的两半（`assemble` 作业会断言）
- [ ] `docs/dev/NOTICE` 的生成块是最新的（`just gen-licenses` 无 diff）
- [ ] `docs/dev/features.md` 的任务卡状态、5.1 追溯表、0.7 总览三处一致
- [ ] `OB-1` 的归属徽章在两个 README 的首屏可见
- [ ] 打 tag 用的提交就是 `main` 的 HEAD（tag 可以指向任何提交，包括没进过 PR 的）

## 2. 首次发布前的预演（只需要做一次）

流水线的 `workflow_dispatch` 触发方式会跑完 `gate` → `source` → `packages` → `assemble`，
**但不会创建 Release**（`publish` 作业有 `if: github.event_name == 'push'`）。第一次正式打
tag 之前，先用它确认整条签名链可用。

```bash
# 在 GitHub 的 Actions 页面选 release 工作流，点 Run workflow；
# 或者用 gh：
gh workflow run release.yml --ref main
```

预演要确认的三件事，都是"只报绿不报范围"最容易掩盖的：

1. **`assemble` 的 `verify` 步骤真的会失败。** 在预演跑出的 `dist/` 里改一个产物的一个字节，
   重跑 `xtask verify`，断言它以 `dist/verify/digest-mismatch` 失败。不这样做，"自校验"只是
   一个从未失败过的步骤。
2. **`packages` 的架构断言真的会失败。** 把矩阵里某一行的 `machine` 改成另一个架构，
   断言作业以非零退出。runner 静默回落时，这是唯一能拦住"标签与产物不符"的地方。
3. **私钥确实被删掉了。** `assemble` 的最后一步断言 runner 的钥匙环里没有 `sec:` 记录；
   日志里不应出现私钥内容（`gpg --import` 的输出里只有 key id 与指纹）。

## 3. 发布后确认（Release 创建之后）

- [ ] Release 的资产清单与 `rspinyin-release.json` 的 `artifacts` 数组一致（`publish` 作业
      的最后一步已经断言：资产 = manifest 的 artifacts + `rspinyin-release.json` +
      `SHA256SUMS` + `SHA256SUMS.asc`）
- [ ] `SHA256SUMS.asc` 能被公钥验证（`assemble` 作业已经断言过自己那份）
- [ ] 在一个干净容器里走一遍用户路径：下载 → `gpg --verify` → `xtask verify` → 安装 →
      输入法可用（这一条**只能人工做**，见 `BUILD-P2.06.01`）

## 4. 失败处置顺序

| 失败作业 | 首先检查 | 常见原因 |
|---|---|---|
| `gate` | tag 与 `Cargo.toml` 的版本是否一致 | 打了 tag 才发现版本没提 |
| `gate` | 描述符版本是否跟着提了 | `xtask check-versions` 会点名哪个 conf 落后 |
| `source` | 源码归档里是否缺文件 | 某个打包配方需要的文件没被 `git` 跟踪（`git archive` 只含已跟踪文件） |
| `packages` | `uname -m` 与矩阵声明是否一致 | runner 静默回落，被架构断言拦住 |
| `packages` | 容器镜像能否拉到、能否装出工具链 | `fedora:42` / `archlinux:base-devel` 的网络或标签变动 |
| `packages` | 包版本与版本号是否一致 | changelog / spec / PKGBUILD 没跟着提 |
| `assemble` | GPG secret 是否配置、是否过期 | 私钥未设、已过期、或公钥文件没提交 |
| `assemble` | manifest 的 `artifacts` 与实际文件是否吻合 | 某架构的产物没被 upload，`xtask release manifest` 会点名缺哪一个角色 |
| `assemble` | `xtask verify` 报的是哪一个 `dist/*` 码 | 见下表；`dist/verify/signing-key-absent` 通常意味着私钥与公钥不是一对 |
| `publish` | `permissions: contents: write` 是否生效 | 仓库的 Actions 权限被收紧 |
| `publish` | `gh release create` 的 glob 是否展开 | 某个产物类别的文件一个都没有（前面已经该失败） |

`xtask verify` 的九个稳定错误码（`docs/dev/opt-deploy.md` 3.3.2，**一经发布不得改写**）：

| 码 | 含义 |
|---|---|
| `dist/manifest/unsupported-version` | manifest 的 schema 版本超出本 build 的理解范围 |
| `dist/manifest/malformed` | JSON 结构或字段类型非法 |
| `dist/verify/artifact-missing` | manifest 列出的产物在目录中不存在 |
| `dist/verify/digest-mismatch` | SHA256 与 manifest 不符，或目录里有文件没有任何记录覆盖 |
| `dist/verify/size-budget-exceeded` | 体积超过 manifest 记录的 `size_budget_mb` |
| `dist/verify/signing-key-absent` | 本地钥匙环没有 `key_id` |
| `dist/verify/signature-invalid` | GPG 校验失败（含过期/吊销的钥匙） |
| `dist/verify/factory-symbol-missing` | `.dynsym` 缺少 manifest `exports` 列出的符号 |
| `dist/verify/dictionary-invalid` | `base.dict` 未通过格式/CRC 校验 |

流水线自身的失败**不使用**这九个码中的任何一个，也不新增第十个：它们说的是"发布没组装
好"，而九个码说的是"用户下载到的东西校验不过"。把两者混起来会让维护者去查一个用户侧问题。

## 5. 一次 Release 的内容

| 文件 | 来源 | 说明 |
|---|---|---|
| `rspinyin-<version>.tar.gz` | `source` 作业 | 源码 tarball（`L-08`）。三个打包配方都从它构建 |
| `rspinyin_<version>-1_amd64.deb` | `packages` 作业（`deb`/`amd64`） | Debian/Ubuntu |
| `rspinyin_<version>-1_arm64.deb` | `packages` 作业（`deb`/`arm64`） | Debian/Ubuntu aarch64 |
| `rspinyin-<version>-1[.<dist>].<arch>.rpm` | `packages` 作业（`rpm`） | Fedora |
| `rspinyin-<version>-1-<arch>.pkg.tar.zst` | `packages` 作业（`pkg`/`x86_64`） | Arch Linux |
| `rspinyin-signing-key.asc` | `assemble` 作业 | 公钥；用户 `gpg --import` 它，不需要 keyserver |
| `rspinyin-release.json` | `xtask release manifest` | 发布清单（`docs/dev/opt-deploy.md` 3.3.1 的 schema） |
| `SHA256SUMS` | `assemble` 作业 | 覆盖 `dist/` 下除自身与签名外的每一个文件 |
| `SHA256SUMS.asc` | `assemble` 作业 | 对 `SHA256SUMS` 的分离签名 |

**`arch` 后缀的拼写分两套**：Debian 用 `amd64` / `arm64`，rpm 与 pacman 用 `x86_64` /
`aarch64`。`xtask release manifest` 按各自的生态拼写识别文件名，而 `--packages` 参数一律用
`uname -m` 的那一套（`x86_64` / `aarch64`）。

## 6. 已知边界与未覆盖项

这些是**如实登记**的边界，不是待办清单上的占位符：

1. **Arch Linux aarch64 没有二进制包。** 官方 `archlinux` 容器镜像只发布 `linux/amd64`
   （`hub.docker.com/v2/repositories/archlinux/archlinux/tags` 的每个 tag 都只有 `amd64`），
   而 Arch 的 aarch64 是另一个项目（ALARM）、另一棵树。因此流水线的 `pkg` 矩阵只有
   `x86_64` 一行，`--packages` 里也只有 `pkg:x86_64`。交付矩阵里 Arch 的产物格式本来就是
   `PKGBUILD`（`docs/dev/opt-deploy.md` 3.1 的 `L-04` / `L-07`），AUR 才是它的分发渠道。
   要补上 aarch64 的 `.pkg.tar.zst`，需要引入一个 ALARM 根文件系统或第三方镜像，那是一次
   供应链决策，需先经用户确认。
2. **没有按架构发布的二进制 tarball。** `xtask package` 产出的
   `rspinyin-<version>-<arch>.tar.gz` 不进 Release：它的载荷（两个 addon、词库、描述符、
   图标）已经在每个发行版包里，而 Release 的文件清单由 `xtask release manifest` 固定。
3. **`assemble` 的 `xtask verify` 不做符号与词库检查。** 发布出来的文件里没有 ELF 镜像、
   也没有 `base.dict`（它们都在包里），所以 S4 状态是空跑。这两项检查发生在更早的位置：
   每个打包配方内部由 `xtask install` 剥离并复检 `fcitx_addon_factory_instance`，体积预算
   也在同一处断言（超预算的产物装不进包）。`xtask package` 自己的 manifest 覆盖的是载荷级
   的检查，那份文档不随 Release 发布。
4. **`publish` 之外没有别的发布渠道。** 没有 AUR 自动推送、没有 PPA、没有 copr：AUR 的
   `sha256sums` 需要人工 `updpkgsums`（`packaging/aur/PKGBUILD` 的注释已经写明 `SKIP` 不是
   可发布值）。
5. **`workflow_dispatch` 预演从不发布。** 这是刻意的：`publish` 是唯一无法回退的一步。

## 7. 密钥

生成、保管、轮换的完整说明在 [`packaging/keys/README.md`](../../packaging/keys/README.md)。
三条硬约束：

- 私钥只经仓库 secret `RSPINYIN_GPG_PRIVATE_KEY` 注入，**绝不落盘、绝不提交、绝不打印**；
  `assemble` 作业在签名后有一个 `if: always()` 的步骤把它从 runner 的钥匙环里删除，并断言
  删除成功。
- 公钥 `packaging/keys/rspinyin-signing-key.asc` 提交进仓库，并作为 Release 资产发布；
  `xtask verify` 要求目录里每个文件都有记录，所以它是 manifest 里的一个 artifact
  （`role: signing-key`）。
- 校验过程不访问 keyserver：`xtask verify` 给 gpg 传了 `--no-auto-key-locate` 与
  `--no-auto-key-retrieve`，这两个开关是"校验一个下载不能联网"这句话的全部实现。
