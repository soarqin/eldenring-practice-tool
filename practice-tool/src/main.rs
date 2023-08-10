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

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::mem;
use std::os::windows::prelude::{AsRawHandle, OsStringExt};
use std::path::PathBuf;

use dll_syringe::process::OwnedProcess;
use dll_syringe::Syringe;
use hudhook::tracing::{debug, trace};
use pkg_version::*;
use semver::*;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
use windows::core::{w, PCSTR, PWSTR};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_FORMAT};
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OPENFILENAMEW, OPEN_FILENAME_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxA, MessageBoxW, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_YESNO,
};

fn err_to_string<T: std::fmt::Display>(e: T) -> String {
    format!("错误: {}", e)
}

fn get_current_version() -> Version {
    Version {
        major: pkg_version_major!(),
        minor: pkg_version_minor!(),
        patch: pkg_version_patch!(),
        pre: Prerelease::EMPTY,
        build: BuildMetadata::EMPTY,
    }
}

fn get_latest_version() -> Result<(Version, String, String), String> {
    #[derive(serde::Deserialize)]
    struct GithubRelease {
        tag_name: String,
        html_url: String,
        body: String,
    }

    let release =
        ureq::get("https://api.github.com/repos/veeenu/eldenring-practice-tool/releases/latest")
            .call()
            .map_err(|e| format!("{}", e))?
            .into_json::<GithubRelease>()
            .map_err(|e| format!("{}", e))?;

    let version = Version::parse(&release.tag_name).map_err(err_to_string)?;

    Ok((version, release.html_url, release.body))
}

fn check_eac(handle: HANDLE) -> Result<bool, String> {
    let mut buf = [0u16; 256];
    let mut len = 256u32;
    let exe_path = PWSTR(buf.as_mut_ptr());
    unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_FORMAT(0), exe_path, &mut len) }
        .map_err(|e| format!("{e}"))?;
    let exe_path = PathBuf::from(unsafe { exe_path.to_string() }.map_err(|e| format!("{e}"))?);
    let exe_cwd = exe_path.parent().unwrap(); // Unwrap ok: must be in a Game directory anyway

    let steam_appid_path = exe_cwd.join("steam_appid.txt");
    debug!("{steam_appid_path:?} {}", steam_appid_path.exists());
    if !steam_appid_path.exists() {
        unsafe {
            let text = w!("如果不绕过EAC启动游戏无法启用练习工具。\n\n\
                           别担心！我们可以帮你搞定他。\n\n请关闭游戏，点击\"Ok\", \
                           然后选择你要绕过EAC的游戏eldenring.exe主文件。");
            let caption = w!("EAC was not bypassed");
            MessageBoxW(HWND(0), text, caption, MB_ICONERROR);

            let mut file_path = [0u16; 256];
            let mut open_file_name = OPENFILENAMEW {
                lStructSize: mem::size_of::<OPENFILENAMEW>() as u32,
                lpstrFilter: w!("Elden Ring 可执行文件 (eldenring.exe)\0eldenring.exe\0\0"),
                nMaxCustFilter: 0,
                nFilterIndex: 0,
                lpstrFile: PWSTR(file_path.as_mut_ptr()),
                nMaxFile: 256,
                nMaxFileTitle: 0,
                Flags: OPEN_FILENAME_FLAGS(0),
                nFileOffset: 0,
                nFileExtension: 0,
                ..Default::default()
            };

            if GetOpenFileNameW(&mut open_file_name).as_bool() {
                let exe_path = PathBuf::from(OsString::from_wide(&file_path));
                // Unwrap ok: must be in a Game directory anyway
                let steam_appid_path = exe_path.parent().unwrap().join("steam_appid.txt");
                let mut file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .open(steam_appid_path)
                    .map_err(|e| format!("无法打开 steam_appid.txt: {e}"))?;
                file.write_all(b"1245620")
                    .map_err(|e| format!("无法写入 steam_appid.txt: {e}"))?;

                let text = w!("已绕过EAC。现在你可以重启游戏并运行练习工具了。");
                let caption = w!("已绕过EAC");
                MessageBoxW(HWND(0), text, caption, MB_ICONINFORMATION);
            } else {
                let text = w!("无法绕过EAC。请重新运行工具，或者手动绕过。\n\
                               请参考: \nhttps://wiki.speedsouls.com/eldenring:EAC_Bypass");
                let caption = w!("未能绕过EAC");
                MessageBoxW(HWND(0), text, caption, MB_ICONERROR);
            }

            return Ok(true);
        }
    }

    Ok(false)
}

fn perform_injection() -> Result<(), String> {
    let mut dll_path = std::env::current_exe().unwrap();
    dll_path.pop();
    dll_path.push("jdsd_er_practice_tool.dll");

    if !dll_path.exists() {
        dll_path.pop();
        dll_path.push("libjdsd_er_practice_tool");
        dll_path.set_extension("dll");
    }

    let dll_path = dll_path.canonicalize().map_err(err_to_string)?;
    trace!("注入 {:?}", dll_path);

    let process = OwnedProcess::find_first_by_name("eldenring.exe")
        .ok_or_else(|| "找不到进程".to_string())?;

    trace!("检查 EAC...");
    if check_eac(HANDLE(process.as_raw_handle() as _))? {
        return Ok(());
    }

    let syringe = Syringe::for_process(process);
    syringe.inject(dll_path).map_err(|e| {
        format!(
            "无法注入练习工具: {e}.\n\n请确认你已经关闭了杀毒软件，绕过了EAC启动游戏，并且运行了未MOD的正版游戏。"
        )
    })?;

    Ok(())
}

fn main() {
    {
        let stdout_layer = tracing_subscriber::fmt::layer()
            .with_thread_ids(true)
            .with_file(true)
            .with_line_number(true)
            .with_thread_names(true)
            .with_ansi(true)
            .boxed();

        tracing_subscriber::registry().with(LevelFilter::TRACE).with(stdout_layer).init();
    }

    let current_version = get_current_version();

    match get_latest_version() {
        Ok((latest_version, download_url, release_notes)) => {
            if latest_version > current_version {
                let update_msg = format!(
                    "练习工具有更新!\n\n最新的版本: \
                     {}\n已安装版本: {}\n\n更新日志:\n{}\n\n你要下载更新吗?\0",
                    latest_version, current_version, release_notes
                );

                let msgbox_response = unsafe {
                    MessageBoxA(
                        HWND(0),
                        PCSTR(update_msg.as_str().as_ptr()),
                        PCSTR("有可用的更新\0".as_ptr()),
                        MB_YESNO | MB_ICONINFORMATION,
                    )
                };

                if IDYES == msgbox_response {
                    open::that(download_url).ok();
                }
            }
        },
        Err(e) => {
            let error_msg = format!("无法检查版本更新: {}\0", e);
            unsafe {
                MessageBoxA(
                    HWND(0),
                    PCSTR(error_msg.as_str().as_ptr()),
                    PCSTR("错误\0".as_ptr()),
                    MB_OK | MB_ICONERROR,
                );
            }
        },
    }

    if let Err(e) = perform_injection() {
        let error_msg = format!("{}\0", e);
        debug!("{e}");
        unsafe {
            MessageBoxA(
                HWND(0),
                PCSTR(error_msg.as_str().as_ptr()),
                PCSTR("错误\0".as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}
