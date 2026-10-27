# testdata/fonts — 测试专用 hermetic 字体夹具

`fonts.conf` 与字体文件共同构成 cargo 全部子进程（含测试）唯一可见的字体库：
`.cargo/config.toml` 通过 `[env]` 把 `FONTCONFIG_FILE` 指向本目录的
`fonts.conf`，fontdb（Slint 软件渲染器的字体源）因此只加载本目录的字体，
与开发机的字体集完全解耦。

## 为什么需要它

Slint 软件渲染器经 fontdb 解析文本；fontdb 在 Linux 上按 fontconfig
列出的目录加载字体。依赖机器字体时渲染测试永远不 hermetic：装有完整
Noto CJK 字体族的机器上，fontdue 在 debug 构建下急切解析 19 MiB 的
CJK 集合字体要 25 秒以上（每进程每线程一次），足以把每个 surface /
addon 测试推过 nextest 的熔断阈值。换成本夹具的 87 KiB 子集后解析在
毫秒级，且字符集覆盖全部测试场景绘制的文本。

## 文件与来源

- `NotoSansCJKSC-testsubset.otf` — Google Noto Sans CJK SC（OFL 1.1）
  子集，family 名保持 `Noto Sans CJK SC`（与 `ime-ui` 探测器的首选
  family 一致，使探测走"健康机器"路径）。
- `LICENSE-OFL.txt` — 上述字体的 SIL Open Font License 1.1 全文。
- `fonts.conf` — 供 `FONTCONFIG_FILE` 指向的 fontconfig 配置。

## 再生成

字符集取自 `crates/ime-ui`、`crates/ime-ui-addon` 的源码（含测试夹具）
与 `crates/ime-ui/ui/*.slint` 中的全部非 ASCII 字符，外加可打印 ASCII：

```bash
grep -rhoP '[^\x00-\x7F]' crates/ime-ui/src/ crates/ime-ui/ui/ crates/ime-ui-addon/src/ \
    | sort -u > /tmp/charset-nonascii.txt
pyftsubset /usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc \
    --font-number=2 \
    --text-file=/tmp/charset-nonascii.txt \
    --output-file=testdata/fonts/NotoSansCJKSC-testsubset.otf \
    --name-IDs='*' --glyph-names --layout-features='*'
```

若测试新增绘制的字符不在子集内，fontdue 会回退到 .notdef 方块（仍有
墨迹、不致挂起）；需要真实字形时按上面的命令重生成即可。
