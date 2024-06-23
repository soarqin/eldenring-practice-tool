欢迎使用艾尔登法环练习工具！

----------------------------------------------------------------------------------------------------
# 如何运行工具

请双击 `eldenring.exe` 启动游戏（Steam → 右键点击「艾尔登法环」→ 管理 → 浏览本地文件）。
不要从 Steam 启动游戏，否则工具无法运行。

有两种运行方式。

直接运行 `jdsd_er_practice_tool.exe`：
  - 将压缩包中的所有文件解压到任意文件夹。
  - 在游戏运行时，双击 `jdsd_er_practice_tool.exe`。

安装到游戏目录：
  - 将所有文件复制到包含 `eldenring.exe` 的游戏目录。
    如果使用多个游戏版本，请分别复制到对应目录。
  - 双击 `eldenring.exe` 启动游戏。
  - 启动时按住右 Shift 键几秒，直到工具出现。
    如果错过启动时机，请双击游戏目录中的 `jdsd_er_practice_tool.exe`。
  - 更新时用新文件覆盖旧文件；卸载时删除 `dinput8.dll`。

工具始终加载所运行的 `jdsd_er_practice_tool.exe` 同目录下的 `dinput8.dll`。
试用新版本时，请先在不按右 Shift 键的情况下启动游戏，再运行新版本的 exe。

首次运行时，如果尚未绕过 EAC（小蓝熊），工具会提示并帮助绕过。
之后请依次重启游戏和工具。

在 Linux 上，请将 Steam 中的游戏启动选项设为 `WINEDLLOVERRIDES="dinput8=n,b" %command%`。
----------------------------------------------------------------------------------------------------

如果遇到问题，请先仔细查看 FAQ。
如果仍未找到答案，欢迎提交 Issue 或加入 Discord 反馈。

FAQ：
https://github.com/veeenu/eldenring-practice-tool#readme

如果喜欢这个工具，请考虑赞助：
https://patreon.com/johndisandonato

-- johndisandonato
