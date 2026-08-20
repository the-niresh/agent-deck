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

    migrate_legacy_asset_dir(&legacy_asset_dir, &new_asset_dir)
        .expect("Failed to prepare Agent Deck asset directory");

    new_asset_dir
}

fn project_data_dir(application: &str) -> std::path::PathBuf {
    ProjectDirs::from("ai", "bloop", application)
        .expect("OS didn't give us a home directory")
        .data_dir()
        .to_path_buf()
}

fn migrate_legacy_asset_dir(legacy_asset_dir: &Path, new_asset_dir: &Path) -> io::Result<()> {
    if new_asset_dir.exists() {
        return Ok(());
    }

    if legacy_asset_dir.exists() {
        tracing::info!(
            from = %legacy_asset_dir.display(),
            to = %new_asset_dir.display(),
            "Migrating application data directory"
        );
        std::fs::rename(legacy_asset_dir, new_asset_dir)?;
    } else {
        std::fs::create_dir_all(new_asset_dir)?;
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
    use std::fs;

    use super::migrate_legacy_asset_dir;

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
}
