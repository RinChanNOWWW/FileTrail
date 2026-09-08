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

如需一次完成 FileTrail 安装和 Tab 补全配置：

```sh
./install.sh
```

脚本根据 `$SHELL` 识别 Bash、Zsh 或 Fish，也可以用 `./install.sh zsh` 显式指定。
它先通过 Cargo 安装，再配置所选 shell 的补全，完成后重新打开 shell 即可。
安装根目录默认为 `${CARGO_HOME:-$HOME/.cargo}`，可通过 `CARGO_INSTALL_ROOT` 覆盖。

## Tab 补全

如果使用 `cargo install` 安装 FileTrail，执行以下命令启用补全：

```sh
filetrail completions --install
```

该命令根据 `$SHELL` 识别 shell，也可以显式指定：

```sh
filetrail completions zsh --install
filetrail completions bash --install
filetrail completions fish --install
```

执行你所用 shell 对应的命令，然后重新打开 shell。Tab 可以补全子命令
（包括 `daemon` 和 `service` 的操作）、选项及文件路径。例如：
`filetrail da<Tab>`、`filetrail daemon st<Tab>`、`filetrail add --f<Tab>`。

安装会保留已有 shell 配置，重复执行不会添加重复配置。配置位置为 `.zshrc`
（遵循 `ZDOTDIR`）、`.bashrc` 和 Bash 当前使用的登录配置文件，或 Fish 的补全目录
（遵循 `XDG_CONFIG_HOME`）。安装的补全配置和命令输出使用 `$HOME` 表示 Home 路径，
不写入用户名；Home 以外的路径保留绝对位置。在相同位置升级可执行文件后，补全会同步更新；
移动可执行文件后需重新安装补全。若要移除补全，删除安装命令所列配置文件中
带有 FileTrail 标记的配置块即可。

如需手动配置，可省略 `--install`，只输出补全脚本：

```sh
filetrail completions zsh
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

Home 内的来源保存在 `__HOME__` 下，保留相对 Home 的路径；其他来源保存在
`__ROOT__` 下，保留去掉开头 `/` 的绝对路径层级。这两个目录位于 `init` 指定的
子目录中；未指定子目录时，直接位于仓库根目录。

| 来源 | 子目录 | 仓库中的文件 |
| --- | --- | --- |
| `~/.zshrc` | `macos` | `macos/__HOME__/.zshrc` |
| `~/.config/nvim` | `linux` | `linux/__HOME__/.config/nvim/init.lua` |
| `/opt/scripts/build.sh` | `macos` | `macos/__ROOT__/opt/scripts/build.sh` |
| `/opt/scripts` | 不指定 | `__ROOT__/opt/scripts/build.sh` |

恢复时，`__HOME__` 对应当前用户的 Home，`__ROOT__` 对应 `/`。
不支持自定义目标路径，保留来源路径才能明确原始位置。`__HOME__` 和 `__ROOT__`
为保留名称，不能作为 `--subdir` 的路径组成部分。

```sh
filetrail add /opt/scripts
filetrail add ~/notes --exclude '**/*.tmp'
```

目录默认递归监听，排除规则相对于来源根目录。相对来源路径以当前目录为基准，
父目录中的符号链接会解析为实际路径。来源、目标和应用数据目录不能相互重叠。

## 更换目标或重新初始化

切换仓库时保留监听项、排除规则和删除设置：

```sh
filetrail retarget ~/new-dotfiles
filetrail retarget ~/new-dotfiles --subdir linux
filetrail retarget ~/new-dotfiles --subdir . # 保存到仓库根目录
```

省略 `--subdir` 时保留当前子目录。指定当前仓库也可以仅更换子目录。
FileTrail 会在需要时创建仓库，并立即将已启用的监听项同步到新位置。
后台任务会继续使用新目标，并保持原来的暂停或运行状态。

旧文件和 Git 历史保留在原处，不会迁移或删除。新目标已有的不同内容会保留并报告为冲突，
由你决定如何处理。即使首次同步报告冲突，目标也已经切换；此时 `conflicts` 和 `resolve`
针对的是新仓库。

不再使用某个配置，或需要从头初始化时：

```sh
filetrail deinit
filetrail init ~/another-repository --subdir macos
```

`deinit` 会停止后台任务、卸载该配置已注册的开机启动服务，并清除配置与同步记录。
源文件、仓库、Git 历史、日志和 shell 补全都会保留，可以安全地重复执行。
重新初始化后，需要再次添加监听项并启动后台任务或安装服务。
如果使用了自定义数据目录，请传入相同的 `--data-dir`。

## 从列表批量添加

```sh
filetrail add --from ./files.txt
```

每行填写一个来源路径，包含空格时使用单引号或双引号包住。
不支持用于自定义目标的第二列。

```text
# source
~/.zshrc
~/.config/nvim
/opt/scripts
"~/My Notes"
'./local scripts'
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
filetrail add ~/scripts --delete
```

只会删除以前成功同步过的文件。如果整个来源目录不可用，FileTrail 会保留目标
文件。符号链接按链接本身复制，不跟随其内容；Git 不记录空目录。`.git` 始终排除，
提交时遵循目标仓库的 Git 忽略规则。

## 查看差异与提交

```sh
filetrail status
filetrail diff
filetrail diff -- macos/__HOME__/.config/nvim
filetrail commit
filetrail commit -m 'Update shell configuration'
filetrail commit -- macos/__HOME__/.zshrc
```

后台不会自动提交或推送。`diff` 包含新增文件的内容；`commit` 只提交受管理的文件，
仓库已有暂存修改时会拒绝提交。首次提交前，请配置好 Git 用户名和邮箱。

不传 `-m` 时，FileTrail 会自动生成列出本次变化的消息：

```text
FileTrail: sync 3 files (+1 ~1 -1)

add "macos/__HOME__/.config/nvim/init.lua"
delete "macos/__HOME__/.oldrc"
modify "macos/__HOME__/.zshrc"
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
filetrail resolve macos/__HOME__/.zshrc --use-source
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
```

开发说明见 [AGENTS.md](AGENTS.md)。
