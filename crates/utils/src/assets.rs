use std::{io, path::Path};

use directories::ProjectDirs;
use rust_embed::RustEmbed;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

pub fn asset_dir() -> std::path::PathBuf {
    let path = if cfg!(debug_assertions) {
        std::path::PathBuf::from(PROJECT_ROOT).join("../../dev_assets")
    } else {
        prod_asset_dir_path()
    };

    // Ensure the directory exists
    if !path.exists() {
        std::fs::create_dir_all(&path).expect("Failed to create asset directory");
    }

    path
    // ✔ macOS → ~/Library/Application Support/MyApp
    // ✔ Linux → ~/.local/share/myapp   (respects XDG_DATA_HOME)
    // ✔ Windows → %APPDATA%\Example\MyApp
}

pub fn prod_asset_dir_path() -> std::path::PathBuf {
    let new_asset_dir = project_data_dir("agent-deck");
    let legacy_asset_dir = project_data_dir("vibe-kanban");

    if let Err(error) = migrate_legacy_asset_dir(&legacy_asset_dir, &new_asset_dir) {
        tracing::error!(
            from = %legacy_asset_dir.display(),
            to = %new_asset_dir.display(),
            error = %error,
            "Failed to prepare Agent Deck asset directory. Move the data directory manually before restarting."
        );
        std::process::exit(1);
    }

    new_asset_dir
}

fn project_data_dir(application: &str) -> std::path::PathBuf {
    ProjectDirs::from("ai", "bloop", application)
        .expect("OS didn't give us a home directory")
        .data_dir()
        .to_path_buf()
}

fn migrate_legacy_asset_dir(legacy_asset_dir: &Path, new_asset_dir: &Path) -> io::Result<()> {
    migrate_legacy_asset_dir_with_rename(legacy_asset_dir, new_asset_dir, |from, to| {
        std::fs::rename(from, to)
    })
}

fn migrate_legacy_asset_dir_with_rename<F>(
    legacy_asset_dir: &Path,
    new_asset_dir: &Path,
    rename: F,
) -> io::Result<()>
where
    F: FnOnce(&Path, &Path) -> io::Result<()>,
{
    if new_asset_dir.exists() {
        return Ok(());
    }

    if legacy_asset_dir.exists() {
        tracing::info!(
            from = %legacy_asset_dir.display(),
            to = %new_asset_dir.display(),
            "Migrating application data directory"
        );
        match rename(legacy_asset_dir, new_asset_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
                copy_dir_all(legacy_asset_dir, new_asset_dir)?;
                std::fs::remove_dir_all(legacy_asset_dir)?;
            }
            Err(error) if new_asset_dir.exists() => return Ok(()),
            Err(error) => return Err(error),
        }
    } else {
        std::fs::create_dir_all(new_asset_dir)?;
    }

    Ok(())
}

fn copy_dir_all(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&source_path, &destination_path)?;
        } else {
            std::fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

pub fn config_path() -> std::path::PathBuf {
    asset_dir().join("config.json")
}

pub fn profiles_path() -> std::path::PathBuf {
    asset_dir().join("profiles.json")
}

pub fn credentials_path() -> std::path::PathBuf {
    asset_dir().join("credentials.json")
}

pub fn trusted_keys_path() -> std::path::PathBuf {
    asset_dir().join("trusted_ed25519_public_keys.json")
}

pub fn server_signing_key_path() -> std::path::PathBuf {
    asset_dir().join("server_ed25519_signing_key")
}

pub fn relay_host_credentials_path() -> std::path::PathBuf {
    asset_dir().join("relay_host_credentials.json")
}

#[derive(RustEmbed)]
#[folder = "../../assets/sounds"]
pub struct SoundAssets;

#[derive(RustEmbed)]
#[folder = "../../assets/scripts"]
pub struct ScriptAssets;

#[cfg(test)]
mod tests {
    use std::{fs, io};

    use super::{migrate_legacy_asset_dir, migrate_legacy_asset_dir_with_rename};

    #[test]
    fn moves_legacy_data_when_new_directory_does_not_exist() {
        let temp_dir = tempfile::tempdir().unwrap();
        let legacy_dir = temp_dir.path().join("vibe-kanban");
        let new_dir = temp_dir.path().join("agent-deck");
        fs::create_dir(&legacy_dir).unwrap();
        fs::write(legacy_dir.join("db.v2.sqlite"), "existing data").unwrap();

        migrate_legacy_asset_dir(&legacy_dir, &new_dir).unwrap();

        assert!(!legacy_dir.exists());
        assert_eq!(
            fs::read_to_string(new_dir.join("db.v2.sqlite")).unwrap(),
            "existing data"
        );
    }

    #[test]
    fn leaves_both_directories_unchanged_when_new_directory_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let legacy_dir = temp_dir.path().join("vibe-kanban");
        let new_dir = temp_dir.path().join("agent-deck");
        fs::create_dir(&legacy_dir).unwrap();
        fs::create_dir(&new_dir).unwrap();
        fs::write(legacy_dir.join("db.v2.sqlite"), "legacy data").unwrap();
        fs::write(new_dir.join("db.v2.sqlite"), "new data").unwrap();

        migrate_legacy_asset_dir(&legacy_dir, &new_dir).unwrap();

        assert_eq!(
            fs::read_to_string(legacy_dir.join("db.v2.sqlite")).unwrap(),
            "legacy data"
        );
        assert_eq!(
            fs::read_to_string(new_dir.join("db.v2.sqlite")).unwrap(),
            "new data"
        );
    }

    #[test]
    fn creates_new_directory_when_neither_directory_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let legacy_dir = temp_dir.path().join("vibe-kanban");
        let new_dir = temp_dir.path().join("agent-deck");

        migrate_legacy_asset_dir(&legacy_dir, &new_dir).unwrap();

        assert!(!legacy_dir.exists());
        assert!(new_dir.is_dir());
    }

    #[test]
    fn copies_legacy_data_when_rename_crosses_filesystems() {
        let temp_dir = tempfile::tempdir().unwrap();
        let legacy_dir = temp_dir.path().join("vibe-kanban");
        let new_dir = temp_dir.path().join("agent-deck");
        fs::create_dir_all(legacy_dir.join("nested")).unwrap();
        fs::write(legacy_dir.join("nested/db.v2.sqlite"), "existing data").unwrap();

        migrate_legacy_asset_dir_with_rename(&legacy_dir, &new_dir, |_, _| {
            Err(io::Error::from(io::ErrorKind::CrossesDevices))
        })
        .unwrap();

        assert!(!legacy_dir.exists());
        assert_eq!(
            fs::read_to_string(new_dir.join("nested/db.v2.sqlite")).unwrap(),
            "existing data"
        );
    }

    #[test]
    fn accepts_a_directory_created_by_a_racing_process() {
        let temp_dir = tempfile::tempdir().unwrap();
        let legacy_dir = temp_dir.path().join("vibe-kanban");
        let new_dir = temp_dir.path().join("agent-deck");
        fs::create_dir(&legacy_dir).unwrap();

        migrate_legacy_asset_dir_with_rename(&legacy_dir, &new_dir, |_, destination| {
            fs::create_dir(destination)?;
            Err(io::Error::new(io::ErrorKind::AlreadyExists, "raced"))
        })
        .unwrap();

        assert!(new_dir.is_dir());
        assert!(legacy_dir.is_dir());
    }
}
