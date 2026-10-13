// Windows 发布版不要闪出控制台窗口; 其它平台照常。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    wxmsg_desktop_lib::run()
}
