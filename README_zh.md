# FileTrail

[English](README.md) | 简体中文

FileTrail 是一个支持 Git 版本管理的文件同步工具。它监听你指定的文件和目录，
将变化同步到本地 Git 仓库，让你查看差异并决定何时提交。

你可以用它管理分散在电脑上的 dotfiles、脚本、笔记等文件，也可以在同一仓库中
分别保存 macOS 和 Linux 的配置。所有功能由一个可执行文件完成，无需额外安装 Git。

## 安装

在项目目录下执行：

```sh
cargo install --path . --locked
filetrail --help
```

## 开始使用

```sh
filetrail init ~/dotfiles --subdir macos
filetrail add ~/.zshrc
filetrail add ~/.config/nvim
filetrail daemon start
```

Linux 上使用 `--subdir linux`；省略 `--subdir` 则保存到仓库根目录。
目标不存在时，FileTrail 会创建目录并初始化 Git。添加来源后会立即复制已有文件，
后台任务负责同步后续变化。

## 选择保存位置

HOME 内的来源保留相对 HOME 的路径；HOME 外的来源保留绝对路径层级，去掉开头的
`/`。使用 `--to` 可以自定义目标位置，它相对于 `init` 时选择的子目录。

| 来源 | 子目录 | 目标 | 仓库中的文件 |
| --- | --- | --- | --- |
| `~/.zshrc` | `macos` | 默认 | `macos/.zshrc` |
| `~/.config/nvim` | `linux` | 默认 | `linux/.config/nvim/init.lua` |
| `/opt/scripts/build.sh` | `macos` | 默认 | `macos/opt/scripts/build.sh` |
| `/opt/scripts` | `macos` | `scripts` | `macos/scripts/build.sh` |

```sh
filetrail add /opt/scripts --to scripts
filetrail add ~/notes --to notes --exclude '**/*.tmp'
```

目录默认递归监听，排除规则相对于来源根目录。相对来源路径以当前目录为基准，
父目录中的符号链接会解析为实际路径。来源、目标和应用数据目录不能相互重叠。

## 从列表批量添加

```sh
filetrail add --from ./files.txt
```

每行写成 `source` 或 `source target`，用空格分隔。路径中包含空格时，使用单引号
或双引号包住。目标位置可选，省略时使用前面介绍的默认规则。

```text
# source [target]
~/.zshrc
~/.config/nvim
/opt/scripts scripts
"~/My Notes" "notes backup"
'./local scripts' 'scripts backup'
```

相对来源路径以列表文件所在目录为基准，支持空行和 `#` 注释。HOME 路径使用 `~`；
列表中的环境变量和命令不会被展开或执行。整个列表检查通过后才会添加，后续编辑
列表不会更新已经导入的条目。

## 管理监听项

```sh
filetrail list
filetrail disable 1
filetrail enable 1
filetrail remove 1
filetrail sync
filetrail sync --dry-run
```

条目 ID 从 `list` 中查看。`remove` 停止监听并保留目标文件。`sync` 立即同步当前
变化，`--dry-run` 只预览将执行的操作。

默认情况下，源文件删除后仍保留目标文件。添加来源时可开启同步删除：

```sh
filetrail add ~/scripts --to scripts --delete
```

只会删除以前成功同步过的文件。如果整个来源目录不可用，FileTrail 会保留目标
文件。符号链接按链接本身复制，不跟随其内容；Git 不记录空目录。`.git` 始终排除，
提交时遵循目标仓库的 Git 忽略规则。

## 查看差异与提交

```sh
filetrail status
filetrail diff
filetrail diff -- macos/.config/nvim
filetrail commit
filetrail commit -m 'Update shell configuration'
filetrail commit -- macos/.zshrc
```

后台不会自动提交或推送。`diff` 包含新增文件的内容；`commit` 只提交受管理的文件，
仓库已有暂存修改时会拒绝提交。首次提交前，请配置好 Git 用户名和邮箱。

不传 `-m` 时，FileTrail 会自动生成列出本次变化的消息：

```text
filetrail: sync 3 files (+1 ~1 -1)

add "macos/.config/nvim/init.lua"
delete "macos/.oldrc"
modify "macos/.zshrc"
```

检查期间需要保持目标内容稳定，可以暂停自动同步：

```sh
filetrail pause
filetrail diff
filetrail commit
filetrail resume
```

`resume` 会补齐暂停期间的变化。显式执行 `sync` 和 `add` 时，即使自动同步已暂停，
仍会复制文件。`diff`、`commit` 和 `resolve` 接收的路径都相对于仓库根目录。

## 处理冲突

首次同步时目标与已有来源内容不同，或者你在 FileTrail 之外修改了目标文件，
FileTrail 会报告冲突。明确希望使用来源版本时执行：

```sh
filetrail conflicts
filetrail resolve macos/.zshrc --use-source
```

也可以自行将两份文件改为相同内容，再运行 `filetrail sync`。仓库正在进行
merge/rebase 或存在 Git 冲突时，需要先处理完成，再恢复同步。

## 后台运行

```sh
filetrail daemon start
filetrail daemon status
filetrail daemon restart
filetrail daemon stop
filetrail daemon run          # 前台运行
filetrail daemon start --poll # 使用定期扫描代替文件事件监听
```

需要登录后自动启动时，先把可执行文件放到固定位置，再安装用户服务：

```sh
filetrail service show
filetrail service install
filetrail service uninstall
```

macOS 使用 launchd，Linux 使用 systemd 用户服务。安装后，使用 `service uninstall`
停止服务并取消自动拉起。

## 数据目录与排查问题

两个平台都默认将应用数据保存在 `$HOME/.filetrail`。同步后的文件与 Git 历史保存在
`init` 指定的仓库。需要使用其他数据目录时，每条命令传入相同的 `--data-dir`：

```sh
filetrail --data-dir ~/filetrail-work init ~/work-dotfiles --subdir macos
filetrail --data-dir ~/filetrail-work daemon start
```

每个仓库使用一套数据目录。遇到问题或需要查看更多用法时，可以执行：

```sh
filetrail doctor
filetrail logs --follow
filetrail --help
filetrail add --help
filetrail completions zsh
```

开发说明见 [AGENTS.md](AGENTS.md)。
