use std::ffi::OsString;
use std::path::PathBuf;

/// The directories where the operating system and the user install fonts.
///
/// - Windows: `%WINDIR%\Fonts` and the per-user `%LOCALAPPDATA%\Microsoft\Windows\Fonts`.
/// - macOS: `/System/Library/Fonts`, `/Library/Fonts` and `~/Library/Fonts`.
/// - Linux and other Unix systems: `fonts` under `$XDG_DATA_HOME` and each
///   `$XDG_DATA_DIRS` entry (by default `~/.local/share/fonts`,
///   `/usr/local/share/fonts` and `/usr/share/fonts`), and `~/.fonts`. These are the
///   directories fontconfig searches by default; fontconfig's configuration files are
///   not read.
///
/// Fonts elsewhere can be added with [`FontBook::add_dir`](crate::FontBook::add_dir).
/// Microsoft Office for Mac, for one, keeps Calibri, Cambria and its other fonts inside
/// its application bundles rather than in a system font directory.
///
/// The directories are not checked for existence. Fonts in subdirectories are found
/// too.
pub fn system_font_dirs() -> Vec<PathBuf> {
    let platform = if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Unix
    };
    font_dirs(platform, |name| std::env::var_os(name))
}

#[derive(Clone, Copy)]
enum Platform {
    Windows,
    MacOs,
    Unix,
}

fn font_dirs(platform: Platform, env: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    // Relative paths in these variables are invalid; the XDG Base Directory
    // Specification says to ignore them.
    let var = |name: &str| {
        env(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let mut dirs = Vec::new();
    match platform {
        Platform::Windows => {
            let windows = var("WINDIR")
                .or_else(|| var("SystemRoot"))
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            dirs.push(windows.join("Fonts"));
            if let Some(local) = var("LOCALAPPDATA") {
                dirs.push(local.join("Microsoft").join("Windows").join("Fonts"));
            }
        }
        Platform::MacOs => {
            dirs.push(PathBuf::from("/System/Library/Fonts"));
            dirs.push(PathBuf::from("/Library/Fonts"));
            if let Some(home) = var("HOME") {
                dirs.push(home.join("Library").join("Fonts"));
            }
        }
        Platform::Unix => {
            let home = var("HOME");
            let data_home = var("XDG_DATA_HOME")
                .or_else(|| home.as_ref().map(|home| home.join(".local").join("share")));
            if let Some(data_home) = data_home {
                dirs.push(data_home.join("fonts"));
            }
            if let Some(home) = &home {
                dirs.push(home.join(".fonts"));
            }
            let data_dirs = env("XDG_DATA_DIRS")
                .filter(|dirs| !dirs.is_empty())
                .unwrap_or_else(|| OsString::from("/usr/local/share:/usr/share"));
            for dir in std::env::split_paths(&data_dirs) {
                if dir.is_absolute() {
                    dirs.push(dir.join("fonts"));
                }
            }
        }
    }
    let mut unique = Vec::new();
    for dir in dirs {
        if !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn dirs(platform: Platform, vars: &[(&str, &str)]) -> Vec<PathBuf> {
        font_dirs(platform, |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[test]
    #[cfg(unix)]
    fn unix_follows_the_xdg_defaults() {
        assert_eq!(
            dirs(Platform::Unix, &[("HOME", "/home/<name>")]),
            [
                Path::new("/home/<name>/.local/share/fonts"),
                Path::new("/home/<name>/.fonts"),
                Path::new("/usr/local/share/fonts"),
                Path::new("/usr/share/fonts"),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn unix_honours_xdg_variables_and_ignores_relative_paths() {
        let found = dirs(
            Platform::Unix,
            &[
                ("HOME", "/home/<name>"),
                ("XDG_DATA_HOME", "relative"),
                ("XDG_DATA_DIRS", "/opt/share:share:/usr/share:/usr/share"),
            ],
        );
        assert_eq!(
            found,
            [
                Path::new("/home/<name>/.local/share/fonts"),
                Path::new("/home/<name>/.fonts"),
                Path::new("/opt/share/fonts"),
                Path::new("/usr/share/fonts"),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn macos_has_system_local_and_user_fonts() {
        assert_eq!(
            dirs(Platform::MacOs, &[("HOME", "/Users/<name>")]),
            [
                Path::new("/System/Library/Fonts"),
                Path::new("/Library/Fonts"),
                Path::new("/Users/<name>/Library/Fonts"),
            ]
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_has_system_and_user_fonts() {
        assert_eq!(
            dirs(
                Platform::Windows,
                &[
                    ("WINDIR", r"D:\Win"),
                    ("LOCALAPPDATA", r"C:\Users\<name>\AppData\Local"),
                ],
            ),
            [
                Path::new(r"D:\Win\Fonts"),
                Path::new(r"C:\Users\<name>\AppData\Local\Microsoft\Windows\Fonts"),
            ]
        );
        assert_eq!(
            dirs(Platform::Windows, &[]),
            [Path::new(r"C:\Windows\Fonts")]
        );
    }
}
