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

use anyhow::{anyhow, Result};
use hudhook::inject::Process;
use hudhook::tracing::debug;
use libjdsd_er_practice_tool::update::Update;
use libjdsd_er_practice_tool::util::*;
use textwrap_macros::dedent;
use windows::Win32::UI::WindowsAndMessaging::*;

fn perform_injection() -> Result<()> {
    debug!("Looking for ELDEN RING process...");
    let process = Process::by_name("eldenring.exe").map_err(|_| {
        anyhow!(dedent!(
            r#"
            找不到艾尔登法环进程。

            请先确认已经启动游戏。
            
            如果游戏已经启动但依然未正常工作，确认遵循以下步骤：
            - 禁用杀毒软件并卸载所有mod，停用所有游戏内覆盖显示工具（RTSS/FRAPS等）
            - 启动Steam (可以离线模式)
            - 双击 eldenring.exe
              (Steam > 艾尔登法环 > 管理 > 浏览本地文件)
            - 双击 jdsd_er_practice_tool.exe
            "#
        )
        .trim())
    })?;

    debug!("Searching for tool DLL...");
    let dll_path = get_dll_path_exe().map_err(|e| {
        anyhow!(
            "Could not find the tool DLL: {e}.\n\nPlease make sure you have extracted the \
             practice tool's zip file contents to a directory before trying again."
        )
    })?;

    debug!("Checking EAC...");
    if check_eac(&process)? {
        return Ok(());
    }

    debug!("Injecting {:?}...", dll_path);
    process.inject(dll_path).map_err(|e| {
        anyhow!(
            "无法注入练习工具: {e}.\n\n请确保你禁用了杀毒软件，\
             绕过了EAC(小蓝熊)，并且运行了未打mod的原版游戏，且\
             停用了所有游戏内覆盖显示工具（RTSS/FRAPS等）。"
        )
    })?;

    Ok(())
}

fn main() -> Result<()> {
    tracing_init();

    match Update::check() {
        Update::Error(e) => {
            let _ = message_box(
                "Elden Ring Practice Tool - Error",
                format!("Could not check for updates: {e}"),
                MB_OK | MB_ICONERROR,
            );
        },
        Update::Available { url, notes } => {
            if let MESSAGEBOX_RESULT(1) = message_box(
                "Elden Ring Practice Tool - Update available",
                format!("{notes}\n\nDo you want to download it?"),
                MB_OKCANCEL | MB_ICONINFORMATION,
            ) {
                return Ok(open::that(&url)?);
            }
        },
        Update::UpToDate => {},
    }

    if let Err(e) = perform_injection() {
        message_box("错误", e.to_string(), MB_OK | MB_ICONERROR);
    }

    Ok(())
}
