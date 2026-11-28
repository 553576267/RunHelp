# 贡献指南

感谢你愿意改进 RunHelp。

## 环境准备

- Node 22+
- Rust 稳定版（Windows 上 MSVC + Windows SDK，或 GNU + MinGW-w64）
- Python 3 + Pillow（仅图片压缩功能需要）

Windows 上若没有 Windows SDK，可使用 GNU 工具链：

```bash
rustup toolchain install stable-x86_64-pc-windows-gnu --profile minimal --force
rustup default stable-x86_64-pc-windows-gnu
export PATH="/path/to/msys64/mingw64/bin:$HOME/.cargo/bin:$PATH"
```

## 本地运行

```bash
npm install
npm run tauri dev
```

前端单独类型检查 / 构建：

```bash
npm run build
# 等效于 vue-tsc --noEmit && vite build
```

后端检查：

```bash
cargo check --manifest-path src-tauri/Cargo.toml
```

## 提交规范

- 提交信息用简短中文或英文描述，格式为 `类型: 说明`，例如 `fix: 修复日志被吞的问题`、`feat: 增加脚本版本对比`。
- 一个提交只做一件事；涉及前端 + 后端改动可放在同一提交。
- 不要提交数据库文件、备份、密钥或本机绝对路径。

## 需要留意的设计约束

1. **Tauri 参数命名**：`invoke` 的顶层参数必须是 camelCase，Rust 端结构体内部字段用 snake_case，二者严格对应，否则运行时报参数缺失。
2. **命令输出按字节解码**：`process.rs` 用 `Utf8StreamDecoder` 逐段容错解码，不要改回 `BufReader::lines()`，否则中文 GBK 字节会让整段日志被吞。
3. **子进程静默运行**：Windows 下 spawn 必须带 `CREATE_NO_WINDOW`，否则弹黑窗口，用户关闭会连带杀掉常驻服务。
4. **内置脚本入库**：`scripts/*.py` 通过 `include_str!` 编译进后端，并在初始化时播种到 `scripts` 表；修改脚本后版本号需递增，否则已有数据库不会更新。
5. **数据库变更要幂等**：`migrations/` 只放初始建表，后续 `ALTER TABLE` 在 `db.rs` 中条件执行，保证老库可平滑升级。

## 发版流程

仓库架构：**GitCode 是主仓**（`origin`），日常提交只 push 到 GitCode；GitHub 与 Gitee 是镜像，由 GitCode 项目设置里的「仓库镜像 → Push」在 push 后单向自动同步，不要在镜像仓库直接改代码。

1. 更新 `CHANGELOG.md` 与 `package.json` / `Cargo.toml` / `tauri.conf.json` 版本号。
   依赖有增删或升级时，重跑 `python scripts/gen_third_party_licenses.py` 刷新 `THIRD-PARTY-LICENSES.md` 并一并提交。
2. 提交后打 tag：`git tag -a v0.1.0 -m "version 0.1.0" && git push origin v0.1.0`（origin 指向 GitCode）。
3. GitCode「仓库镜像 → Push」把代码和 tag 同步到 GitHub，GitHub Actions 构建 NSIS 安装包并**直接发布正式 Release**（workflow 里 `releaseDraft: false`），无需再到网页手动点发布。
4. 把安装包复制到 GitCode / Gitee 镜像仓库的「发行版」，方便国内用户下载。

## 贡献授权（inbound = outbound）

本项目不需要签署 CLA。提交 Pull Request、补丁或任何形式的代码 / 文档内容，即表示你同意以下四点：

1. 你对所提交内容拥有版权，或已取得版权所有者的授权；
2. 你以**与本项目相同的 MIT 协议**授权该贡献（inbound = outbound），无需额外签署文件；
3. 你同意维护者在不改变「源码继续开源」的前提下，对本项目整体（含你的贡献）做协议版本升级或补充例外条款——例如将来需要一并调整协议以适配某个分发渠道；
4. 你的贡献不包含你无权开源的第三方代码；若引用了外部实现，请在 PR 描述里注明来源与许可。

这样做的原因很实际：没有明确的授权链，将来既无法把协议从 MIT 调整为其它开源协议，也无法在商用或商店上架时证明代码来源干净。若你不接受第 3 点，请在 PR 里注明，我们可以只合并第 1、2 点的授权范围。

## 安全问题

发现安全漏洞**不要**开公开 Issue 贴细节，请按 [SECURITY.md](SECURITY.md) 的流程私下报告。

## 问题反馈

用 Issue 模板提交，附上系统版本、RunHelp 版本、复现步骤与日志。涉及命令执行的问题，请把底部「运行日志」展开后的完整输出贴上。
