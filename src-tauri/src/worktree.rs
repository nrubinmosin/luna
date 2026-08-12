use std::path::Path;
use std::process::Command;

fn norm(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

fn git(folder: &str) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(folder);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

#[tauri::command]
pub fn remove_worktree(folder: String, worktree_path: String) -> Result<(), String> {
    let base = Path::new(&folder).join(".claude").join("worktrees");
    if !norm(&worktree_path).starts_with(&norm(&base.to_string_lossy())) {
        return Err("refusing: path is not a worktree of this folder".into());
    }
    if !Path::new(&worktree_path).exists() {
        return Ok(());
    }

    // The killed session may hold file locks for a moment — retry briefly.
    for _ in 0..3 {
        match git(&folder)
            .args(["worktree", "remove", "--force", &worktree_path])
            .output()
        {
            Ok(o) if o.status.success() => return Ok(()),
            _ => std::thread::sleep(std::time::Duration::from_millis(500)),
        }
    }

    std::fs::remove_dir_all(&worktree_path).map_err(|e| e.to_string())?;
    let _ = git(&folder).args(["worktree", "prune"]).output();
    Ok(())
}
