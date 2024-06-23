// johndisandonato's Elden Ring Practice Tool
// Copyright (C) 2022  johndisandonato <https://github.com/veeenu>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use anyhow::{anyhow, bail, Result};
use hudhook::inject::Process;
use hudhook::tracing::debug;
use libjdsd_er_practice_tool::util::*;
use textwrap_macros::dedent;
use windows::Win32::System::Threading::{TerminateProcess, WaitForSingleObjectEx};
use windows::Win32::UI::WindowsAndMessaging::*;

fn install() -> Result<()> {
    message_box(
        "johndisandonato的艾尔登法环练习工具",
        "欢迎使用艾尔登法环练习工具安装器!\n\n如果还没启动艾尔登法环请先启动。\n\n \
         如果你安装了多个版本的法环 (通常是为了速通快速切换版本)，\n\n \
         你必须为每个版本运行一次本安装器。\n\n \
         安装器会在游戏可执行文件所在目录创建一个 `dinput8.dll`，如果要卸载练习工具， \
         只需要删除这个文件。\n\n那么我们开始吧！"
            .trim(),
        MB_OK,
    );

    debug!("Looking for ELDEN RING process...");
    let process = Process::by_name("eldenring.exe").map_err(|_| {
        anyhow!(dedent!(
            r#"
            找不到艾尔登法环进程。

            请先确认已经启动游戏。
            
            如果游戏已经启动，确认遵循以下步骤：
            - 禁用杀毒软件并卸载所有mod
            - 启动Steam (可以离线模式)
            - 双击 eldenring.exe
              (Steam > 艾尔登法环 > 管理 > 浏览本地文件)
            - 双击 jdsd_er_practice_tool.exe
            "#
        )
        .trim())
    })?;

    debug!("Searching for tool DLL...");
    let dll_path = get_dll_path_exe()?;

    let config_path = dll_path
        .parent()
        .ok_or_else(|| anyhow!("{dll_path:?} 没有父目录"))?
        .join("jdsd_er_practice_tool.toml");

    if !config_path.exists() {
        bail!(
            "找不到 jdsd_er_practice_tool.toml。\n请确保运行安装器前你已经解压了完整的zip内容。"
        );
    }

    let game_install_path = get_game_directory(&process)?;

    debug!("Checking EAC...");
    if check_eac(&process)? {
        return Ok(());
    }

    debug!("Installing...");
    let dll_path_dest = game_install_path.join("dinput8.dll");
    let config_path_dest = game_install_path.join("jdsd_er_practice_tool.toml");

    if dll_path_dest.exists() {
        if message_box(
            "关闭艾尔登法环",
            "看起来当前艾尔登法环已经安装了练习工具。\n\n现在将关闭游戏以继续安装过程。请确保你已经 \
             退回到主菜单，并且点击了 \"确定\".\n\n如果你现在不想关闭游戏，请点击 \"取消\" 中断安装。",
            MB_OKCANCEL | MB_ICONINFORMATION,
        ) != MESSAGEBOX_RESULT(1)
        {
            debug!("Aborting installation");
            return Ok(());
        } else {
            unsafe { TerminateProcess(process.handle(), 1) }
                .map_err(|e| anyhow!("无法关闭艾尔登法环: {e}"))?;
            unsafe { WaitForSingleObjectEx(process.handle(), 20000, false) };
        }
    }

    std::fs::copy(&dll_path, &dll_path_dest).map_err(|e| {
        anyhow!(
            "无法装DLL: {e}\n当尝试复制\n{dll_path:?}\n到\n{dll_path_dest:?}时"
        )
    })?;
    std::fs::copy(&config_path, &config_path_dest).map_err(|e| {
        anyhow!(
            "无法安装设置文件: {e}\n当尝试 \
             复制\n{config_path:?}\n到\n{config_path_dest:?}时"
        )
    })?;

    message_box(
        "成功",
        "练习工具安装成功。\n\n要使用练习工具，请重启游戏并按住右Shift数秒直到工具界面出现。\n\n \
         祝你游戏愉快！",
        MB_ICONINFORMATION,
    );

    Ok(())
}

fn main() -> Result<()> {
    tracing_init();

    if let Err(e) = install() {
        message_box("错误", e.to_string(), MB_OK | MB_ICONERROR);
    }

    Ok(())
}
