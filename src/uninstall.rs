//! 一键卸载：删除程序文件，可选把数据目录移到回收站，Windows 上顺带删掉 SUCARD_DIR 环境变量。
//!
//! 安全规则（宁可少删，不可多删）：
//! - 程序：只删 exe 本身；只有 exe 所在文件夹名以 `sucard-` 开头（下载解压出来的那个）时，
//!   才顺带删同目录的 README.md、LICENSE-MIT、LICENSE-APACHE，并在删空后删掉这个文件夹。
//! - 数据：只有目录里有 `.sucard`、且不是磁盘根目录、不是用户目录本身时才允许删，删时移到回收站。

use std::path::{Path, PathBuf};

/// 卸载要做的事。先算好，界面照着显示，确认后照着执行
#[derive(Debug, PartialEq)]
pub struct Plan {
    /// 要删的程序文件（exe 和随它一起下载的说明、许可证）
    pub files: Vec<PathBuf>,
    /// 文件删完后，如果空了就删掉的文件夹
    pub remove_dir: Option<PathBuf>,
    pub data_dir: PathBuf,
    /// 数据目录能不能删；不能删时是原因
    pub data_ok: Result<(), String>,
    /// 设置了 SUCARD_DIR 环境变量
    pub env_var: bool,
}

const RELEASE_FILES: [&str; 3] = ["README.md", "LICENSE-MIT", "LICENSE-APACHE"];

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

pub fn plan(exe: &Path, data_dir: &Path, home: Option<&Path>, env_var: bool) -> Plan {
    let mut files = vec![exe.to_path_buf()];
    let mut remove_dir = None;
    if let Some(dir) = exe.parent() {
        let from_release = dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("sucard-"));
        if from_release {
            files.extend(RELEASE_FILES.iter().map(|f| dir.join(f)).filter(|p| p.is_file()));
            remove_dir = Some(dir.to_path_buf());
        }
    }
    let data_ok = if !data_dir.join(".sucard").is_dir() {
        Err("这个文件夹里没有 .sucard，看起来不是素签的数据目录，不会删除".to_string())
    } else if data_dir.parent().is_none_or(|p| p.as_os_str().is_empty()) {
        Err("数据目录是磁盘根目录，不能整个删除".to_string())
    } else if home.is_some_and(|h| same_path(h, data_dir)) {
        Err("数据目录就是用户目录本身，不能整个删除".to_string())
    } else {
        Ok(())
    };
    Plan { files, remove_dir, data_dir: data_dir.to_path_buf(), data_ok, env_var }
}

/// 执行卸载。数据先移到回收站（失败就整个停下，什么都不删），
/// 然后启动一个后台命令：等本程序退出两三秒后删除程序文件（运行中的 exe 不能删自己）。
/// 成功后调用方应立即关闭程序。
pub fn run(plan: &Plan, delete_data: bool) -> Result<(), String> {
    if delete_data {
        plan.data_ok.clone()?;
        trash::delete(&plan.data_dir).map_err(|e| format!("把数据目录移到回收站失败：{e}"))?;
    }
    #[cfg(windows)]
    if plan.env_var {
        remove_env_var_windows();
    }
    spawn_deleter(&plan.files, plan.remove_dir.as_deref())
}

#[cfg(windows)]
const NO_WINDOW: u32 = 0x0800_0000;

/// 删掉当前用户的 SUCARD_DIR 环境变量（系统级的需要管理员权限，删不掉就算了）
#[cfg(windows)]
fn remove_env_var_windows() {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("reg")
        .args(["delete", r"HKCU\Environment", "/v", "SUCARD_DIR", "/f"])
        .creation_flags(NO_WINDOW)
        .status();
}

/// 路径通过环境变量传给命令，不拼进命令文本，避免路径里的特殊字符出问题
#[cfg(windows)]
fn spawn_deleter(files: &[PathBuf], dir: Option<&Path>) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let mut script = String::from("ping 127.0.0.1 -n 3 >nul");
    let mut cmd = std::process::Command::new("cmd");
    for (i, f) in files.iter().enumerate() {
        let k = format!("SUCARD_DEL_{i}");
        script.push_str(&format!(" & del /f /q \"%{k}%\""));
        cmd.env(k, f);
    }
    if let Some(d) = dir {
        // rmdir 不带 /s：文件夹里还有别的东西就不删
        script.push_str(" & rmdir \"%SUCARD_DEL_DIR%\"");
        cmd.env("SUCARD_DEL_DIR", d);
    }
    cmd.raw_arg(format!("/C {script}"))
        .creation_flags(NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动删除程序失败：{e}"))
}

#[cfg(not(windows))]
fn spawn_deleter(files: &[PathBuf], dir: Option<&Path>) -> Result<(), String> {
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c")
        .arg(r#"sleep 2; rm -f -- "$@"; [ -n "$SUCARD_DEL_DIR" ] && rmdir -- "$SUCARD_DEL_DIR" 2>/dev/null; exit 0"#)
        .arg("sh")
        .args(files);
    if let Some(d) = dir {
        cmd.env("SUCARD_DEL_DIR", d);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("启动删除程序失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sucard-uninstall-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn program_files_only_from_release_folder() {
        let root = temp("prog");
        // 下载解压出来的文件夹：exe + 说明 + 许可证都删，删空后删文件夹
        let rel = root.join("sucard-v0.1.2-x86_64-pc-windows-msvc");
        std::fs::create_dir_all(&rel).unwrap();
        for f in ["sucard.exe", "README.md", "LICENSE-MIT", "LICENSE-APACHE"] {
            std::fs::write(rel.join(f), "x").unwrap();
        }
        let p = plan(&rel.join("sucard.exe"), &root, None, false);
        assert_eq!(p.files.len(), 4);
        assert_eq!(p.remove_dir.as_deref(), Some(rel.as_path()));

        // 放在别的文件夹（比如桌面）：只删 exe，旁边的 README 不动，文件夹不删
        let desk = root.join("Desktop");
        std::fs::create_dir_all(&desk).unwrap();
        std::fs::write(desk.join("README.md"), "我的笔记").unwrap();
        let p = plan(&desk.join("sucard.exe"), &root, None, false);
        assert_eq!(p.files, vec![desk.join("sucard.exe")]);
        assert_eq!(p.remove_dir, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn data_dir_safety() {
        let root = temp("data");
        let exe = root.join("sucard.exe");
        // 正常的数据目录
        let data = root.join("Sucard");
        std::fs::create_dir_all(data.join(".sucard")).unwrap();
        assert!(plan(&exe, &data, Some(&root), false).data_ok.is_ok());
        // 没有 .sucard：不删
        let other = root.join("别的文件夹");
        std::fs::create_dir_all(&other).unwrap();
        assert!(plan(&exe, &other, Some(&root), false).data_ok.is_err());
        // 用户目录本身被当成数据目录：不删
        std::fs::create_dir_all(root.join(".sucard")).unwrap();
        assert!(plan(&exe, &root, Some(&root), false).data_ok.unwrap_err().contains("用户目录"));
        // 磁盘根目录：不删
        let disk_root = if cfg!(windows) { PathBuf::from(r"C:\") } else { PathBuf::from("/") };
        let p = plan(&exe, &disk_root, None, false);
        assert!(p.data_ok.is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真的启动后台删除命令（要等几秒，平时不跑）：
    /// cargo test deleter_really -- --ignored
    #[test]
    #[ignore]
    fn deleter_really_deletes() {
        let root = temp("real 空格 & 50% 中文");
        // 解压出来的文件夹：全删，文件夹也删
        let rel = root.join("sucard-v9 测试");
        std::fs::create_dir_all(&rel).unwrap();
        for f in ["sucard.exe", "README.md", "LICENSE-MIT", "LICENSE-APACHE"] {
            std::fs::write(rel.join(f), "x").unwrap();
        }
        // 同名文件夹里还有用户自己的东西：只删我们的，文件夹和用户文件保留
        let rel2 = root.join("sucard-v9 有别的");
        std::fs::create_dir_all(&rel2).unwrap();
        for f in ["sucard.exe", "README.md", "我的.txt"] {
            std::fs::write(rel2.join(f), "x").unwrap();
        }
        run(&plan(&rel.join("sucard.exe"), &root, None, false), false).unwrap();
        run(&plan(&rel2.join("sucard.exe"), &root, None, false), false).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(6));
        assert!(!rel.exists(), "解压出来的文件夹应该被删掉");
        assert!(!rel2.join("sucard.exe").exists() && !rel2.join("README.md").exists());
        assert!(rel2.join("我的.txt").exists(), "用户自己的文件不能删");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_unsafe_data_dir_without_deleting_anything() {
        let root = temp("refuse");
        let exe = root.join("sucard.exe");
        std::fs::write(&exe, "x").unwrap();
        let p = plan(&exe, &root.join("没有这个目录"), None, false);
        assert!(run(&p, true).is_err());
        assert!(exe.exists(), "数据目录不能删时，程序也不能删");
        let _ = std::fs::remove_dir_all(&root);
    }
}
