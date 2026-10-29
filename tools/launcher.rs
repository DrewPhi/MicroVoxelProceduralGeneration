//! Tiny native desktop entry point. Always starts the current release build,
//! with the project as its working directory and diagnostics in output/.
use std::{
    env,
    fs::{self, File},
    path::PathBuf,
    process::{Command, Stdio},
};
fn launch() -> Result<(), String> {
    let executable = env::current_exe().map_err(|e| e.to_string())?;
    let root = executable
        .parent()
        .ok_or("Cannot locate the project directory")?;
    let game = root.join("native/target/release/microvoxel-game");
    if !game.is_file() {
        return Err("The game needs building first. Run tools/build_launcher.sh from the project directory with Rust 1.85 or newer installed.".into());
    }
    fs::create_dir_all(root.join("output")).map_err(|e| e.to_string())?;
    let log = File::create(root.join("output/launcher.log")).map_err(|e| e.to_string())?;
    let status = Command::new(game)
        .current_dir(root)
        .args(env::args_os().skip(1))
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .status()
        .map_err(|e| format!("Could not start MicroVoxel: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "MicroVoxel exited with {status}. See {}",
            root.join("output/launcher.log").display()
        ))
    }
}
fn main() {
    if let Err(message) = launch() {
        eprintln!("{message}");
        let offline = env::args().skip(1).any(|a| {
            matches!(
                a.as_str(),
                "--headless"
                    | "--help"
                    | "--lod-test"
                    | "--player-test"
                    | "--benchmark"
                    | "--isometric"
                    | "--character-preview"
            )
        });
        if !offline {
            let _ = Command::new("zenity")
                .args(["--error", "--title=MicroVoxel", "--text", &message])
                .status();
        }
        // File-manager launches may have no terminal or optional dialog utility.
        if let Ok(path) = env::current_exe() {
            let root = path.parent().map(PathBuf::from);
            if let Some(root) = root {
                let _ = fs::create_dir_all(root.join("output"));
                let _ = fs::write(root.join("output/launcher-error.txt"), message);
            }
        }
        std::process::exit(1);
    }
}
