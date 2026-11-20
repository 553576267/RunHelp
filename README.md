# RunHelp 开发助手

Windows 桌面端开发辅助工具：管理多个项目的常用命令、一键流水线、图片压缩，数据全部存于本地 SQLite。

技术栈：Tauri 2 + Vue 3 + TypeScript（前端）+ Rust（后端）+ SQLite（rusqlite，单文件）。

## 功能

- **项目管理**：先选类型再新建（开发项目必填路径，图片压缩项目免路径）；页签按类型显示（开发只显示命令中心，图片压缩只显示图片压缩）；删除进回收站，可还原/彻底删除/清空
- **命令中心**：为每个项目添加安装、构建、运行或自定义命令，可选 Shell（PowerShell / CMD / Git Bash）、工作目录、超时
- **流水线**：把多个命令与"等待"步骤串成一键操作，支持失败即停/继续、暂停/继续/终止
- **实时日志**：底部运行日志默认收缩，点标题栏即可展开；支持「只看错误」筛选并显示错误条数；运行中状态会被准确区分（成功/失败/超时/已停止）
- **图片压缩**：可调质量/尺寸/格式/命名/元数据等参数并保存为预设；每个预设可指定"使用脚本"；选择文件或目录后一键调用 Python 脚本批量压缩，显示进度与节省空间；支持收藏常用目录
- **脚本管理**：Python 脚本源码可存入数据库（带版本号），也可外链本地 `.py` 文件；在图片压缩预设里选择后生效
- **备份与恢复**：内置备份脚本，设置页一键备份到指定目录；恢复前自动另存一份保命备份，避免误操作丢数据
- **主题**：深色 / 浅色切换，原生标题栏跟随主题
- **单实例**：重复点击图标只激活已有窗口，不会开多个实例

## 界面预览

**命令中心**：按项目组织安装 / 构建 / 运行命令，底部运行日志默认折叠、可展开，支持只看错误

![命令中心](docs/screenshots/command-center.png)

**图片压缩**：参数预设 + 内置压缩脚本，支持常用目录收藏与进度显示

![图片压缩](docs/screenshots/image-compress.png)

**全局任务**：跨项目查看运行中任务，可暂停 / 终止常驻服务，并保留运行记录

![全局任务](docs/screenshots/tasks.png)

## 下载安装

- **GitCode（主仓，国内推荐）**：`https://gitcode.com/binglengdexie/RunHelp/releases`
- **GitHub 镜像**：`https://github.com/binglengdexie/RunHelp/releases`
- **Gitee 镜像**：`https://gitee.com/binglengdexie/RunHelp/releases`

下载 `RunHelp_x.x.x_x64-setup.exe` 直接安装即可。三个仓库内容一致：**GitCode 是主仓**，日常开发只推 GitCode；GitHub 与 Gitee 由 GitCode 内置的「仓库镜像 → Push」单向自动同步，仅作为其他地区的访问入口。

## 开发

前置要求：Node 22+、Rust（MSVC 或 GNU 工具链均可）、对应平台的 C++ 链接器（Windows 上 MSVC 需 Windows SDK，或改用 GNU + MinGW-w64）、Python 3（仅压缩功能需要）。

```bash
npm install                 # 安装前端依赖
npm run tauri dev           # 开发模式运行
npm run build               # 仅构建前端（含类型检查）
npm run tauri build         # 打包 MSI/NSIS 安装包
```

图标由 `scripts/gen_icons.py` 生成（纯标准库）。

## 图片压缩脚本

内置脚本 `scripts/compress_images.py` 基于 Pillow，运行时由 Rust 端写入临时目录并以 JSON 配置调用：

```bash
pip install Pillow
python compress_images.py --config config.json
```

脚本按行输出 JSON（`progress` / `result` / `error` / `summary`），App 解析后更新进度条与结果列表。脚本内置到 `scripts` 表，用户可在"全局设置 → 脚本管理"里修改源码（修改后版本号 +1）。

**自定义脚本的使用流程**：全局设置 → 脚本管理 → 新建脚本（存入数据库或外链 `.py`）→ 图片压缩页 → 参数预设的"使用脚本"下拉选中它 → 保存预设 → 开始压缩。自定义脚本需遵循内置脚本的协议：读取 `--config` 指向的 JSON（含 `input_paths` 与各项参数），按行输出 `progress`/`result`/`summary`/`error` JSON。

## 数据库位置与备份

默认数据库在 `%APPDATA%\com.runhelp.desktop\runhelp.db`，「全局设置 → 数据库位置」只读显示当前库文件与所在目录。全部项目、命令、脚本、预设与运行记录都在这一个文件里。

**跨设备 / 多电脑同步**：采用「本地库 + 备份文件」方案，不要把数据库本体放进坚果云 / OneDrive 等同步盘（多端同时写会导致损坏）。推荐做法：

- 在「全局设置 → 备份与恢复」把备份目录设为同步盘的某个文件夹，点「立即备份」生成带机器名和时间戳的备份。
- 另一台电脑从同步盘取备份，在同一页面点「恢复」；恢复前会自动把当前库另存为 `.before-restore_时间戳.db`，确认无误后再删除这份保命文件。
- 备份脚本内置在数据库中（也可在 `scripts/backup_db.py` 查看源码），使用 SQLite backup API 热备份，无需退出软件，备份后自动切回 DELETE 模式并校验完整性。

> 数据库为单文件（含 WAL 模式附属文件），备份时请连同 `-wal`/`-shm` 一起，或直接用软件内的「立即备份」让脚本处理。

## Windows 工具链说明

优先用 **MSVC 工具链**（`rustup default stable-x86_64-pc-windows-msvc`），需安装 Visual Studio Build Tools + Windows SDK。GitHub Actions 的打包环境就是 MSVC，无需额外配置。

如果本机没有 Windows SDK，MSVC 会在链接阶段报 `link.exe failed` / `extra operand`，此时可改用 **Rust GNU 工具链 + MSYS2 的 MinGW-w64**（本项目开发机实测通过）：

**1. 安装 MinGW-w64**（假设 MSYS2 装在 `<MSYS2 安装目录>`， pacman 建议换国内源）：
```bash
<MSYS2 安装目录>\usr\bin\bash.exe -lc "pacman -S --noconfirm mingw-w64-x86_64-gcc"
# 装完验证：gcc -dumpmachine 应输出 x86_64-w64-mingw32
```

**2. 安装 Rust GNU 工具链**（必须一次装完整，中途中断会留下缺 manifest 的空壳，导致 `rustc -vV` 失败）：
```bash
rustup toolchain install stable-x86_64-pc-windows-gnu --profile minimal --force
rustup default stable-x86_64-pc-windows-gnu
```

**3. 编译时把 MinGW 的 bin 注入 PATH**（无需改系统 PATH，`<MSYS2 安装目录>` 在 Git Bash 下写成 `/d/xxx/msys64` 这类 POSIX 路径）：
```bash
export PATH="/path/to/msys64/mingw64/bin:$HOME/.cargo/bin:$PATH"
npm run tauri dev      # 或 npm run tauri build
```

> 注意：国内网络下 `npm` 源可用 `https://registry.npmmirror.com`，旧的 `registry.npm.taobao.org` 已废弃会导致安装卡死。

## 目录结构

```
src/                      Vue 前端
  components/             各功能组件
  lib/api.ts              Tauri invoke 封装（参数名与 Rust 端严格一致）
src-tauri/                Rust 后端
  src/commands/           各业务命令（project/command/pipeline/script/settings/compress）
  src/process.rs          进程 spawn、实时日志、终止进程树、暂停等待
  src/state.rs            全局运行时状态（pid 表、暂停标志）
  src/db.rs               数据库初始化与内置脚本播种
migrations/001_init.sql   建表与默认配置
migrations/002_recycle.sql 回收站索引（ADD COLUMN 在 db.rs 条件执行，保证幂等）
scripts/                  Python 压缩脚本、图标生成脚本
```

## 命令执行的设计要点（踩坑记录）

Windows GUI 应用里"跑命令"有几个非常隐蔽的坑，本项目的实现参考了同类开源项目
`ocampoje17/projects-runner-app`（同为 Tauri 2），以下几条务必保持：

1. **输出必须按字节容错解码，不能用 `BufReader::lines()`**。
   `lines()` 要求严格 UTF-8，遇到非法字节（如中文控制台的 GBK 字节、被 read 边界切断的
   多字节字符）会返回 `Err`，旧代码直接 `break`，导致整段 stdout/stderr 被吞——表现为
   "命令失败了但控制台什么错误都没有"。现在 `process.rs` 里的 `Utf8StreamDecoder` 逐段解码，
   坏字节替换为 `U+FFFD` 后继续，永不中断；同时给 Python 子进程加 `PYTHONIOENCODING=utf-8`，
   从源头保证管道输出是 UTF-8。

2. **启动时要修复 PATH**。Windows 下 GUI 程序从资源管理器/其他程序启动时，
   `std::process::Command` 继承的 PATH 可能是陈旧快照。`lib.rs` 的 `run()` 开头把 PATH
   读出再 `set_var` 写回，强制刷新（等价于官方 `fix-path-env` crate）。

3. **不要裸用 `python`，要显式指定解释器**。系统里常有 WorkBuddy/WindowsApps 注入的
   `python` 抢占 PATH（表现为 `No module named 'fastapi'`）。设置页的"Python 解释器路径"
   会把该解释器目录前置到子进程 PATH。`python_path`、`PYTHONUNBUFFERED`、
   `PYTHONIOENCODING` 三者配合，才能既用对环境、又实时看到中文日志。

4. **日志可能被事件时序丢掉**。秒退进程的流式日志可能晚于前端的"清空日志"动作。
   现在后端在 `run://end` 事件里附带完整 stderr 全文，前端据此兜底补齐（自动去重）。

5. **子进程要加 `CREATE_NO_WINDOW`**。Windows 下 GUI 程序 spawn cmd/powershell 会弹黑窗口，
   用户关掉它等于给进程组发 `CTRL_CLOSE`，把常驻的 python 服务一起杀掉（退出码
   `-1073741510`）。`process.rs` 的 `execute()` 用 `CommandExt::creation_flags` 加了
   `CREATE_NO_WINDOW`，子进程静默运行，stdout/stderr 仍走管道捕获。

6. **区分"停止"与"失败"**。用户主动终止（点"终止"按钮或关窗口导致的 `-1073741510`）
   应记为 `stopped`（显示"已停止"），而不是 `failed`。`AppState.runtime` 里维护
   `run_id -> history_id` 映射和"主动终止集合"；应用启动时把遗留的 `running` 记录
   统一收尾为 `stopped`（清理僵尸记录）；应用退出时 kill 所有 tracked 子进程，
   避免服务残留占用端口。

> 提示：常驻服务（如 `python server.py`）的命令类型应选"运行"，超时设 `0`（不限），
> 否则到点会被强制终止（退出码 124）。运行后软件会一直显示"运行中"，需要结束时
> 点"终止"按钮——这是正常行为，不是卡住。

## 开源协议

MIT License，详见 [LICENSE](LICENSE)。可自由使用、修改、商用，无需付费，但需保留版权声明。

## 贡献

欢迎提交 Issue 与 Pull Request，流程见 [CONTRIBUTING.md](CONTRIBUTING.md)。

更新记录见 [CHANGELOG.md](CHANGELOG.md)。
