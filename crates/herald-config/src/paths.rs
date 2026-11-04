use std::path::{Component, Path, PathBuf};

use directories::{BaseDirs, ProjectDirs};

use crate::ConfigError;

/// Where the configuration file lives by default.
#[must_use]
pub fn default_config_path() -> PathBuf {
    ProjectDirs::from("dev", "Herald", "herald").map_or_else(
        || {
            BaseDirs::new().map_or_else(
                || PathBuf::from("herald.toml"),
                |dirs| {
                    dirs.home_dir()
                        .join(".config")
                        .join("herald")
                        .join("config.toml")
                },
            )
        },
        |dirs| dirs.config_dir().join("config.toml"),
    )
}

/// Where Herald stores its database and models by default.
#[must_use]
pub fn default_data_dir() -> Option<PathBuf> {
    ProjectDirs::from("dev", "Herald", "herald").map(|dirs| dirs.data_dir().to_path_buf())
}

/// Expand a leading `~` without consulting the filesystem.
#[must_use]
pub fn expand_tilde(path: &Path, home: &Path) -> PathBuf {
    let mut components = path.components();
    if components.next() == Some(Component::Normal(std::ffi::OsStr::new("~"))) {
        let rest: PathBuf = components.collect();
        return home.join(rest);
    }
    path.to_path_buf()
}

/// Create a directory readable only by the current user.
pub fn ensure_private_dir(path: &Path) -> Result<(), ConfigError> {
    std::fs::create_dir_all(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .map_err(|source| ConfigError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }

    Ok(())
}

#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}
